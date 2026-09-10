use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Json, Path, Query, State},
    http::{header, StatusCode, Uri},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(debug_assertions)]
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::audit;
use crate::audit_events;
use crate::auth;
use crate::config::limits;
use crate::crypto;
use crate::extractors::{AdminUser, AuthUser};
use crate::key_manager::{self, CreateKeyRequest};
use crate::server_registry::{self, CreateServerRequest};
use crate::ssh_bridge;
use crate::ws_handler;
use crate::AppState;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;

#[derive(Serialize)]
pub struct ApiError {
    pub error: String,
}

#[allow(dead_code)]
fn err(status: StatusCode, msg: &str) -> impl IntoResponse {
    (
        status,
        Json(ApiError {
            error: msg.to_string(),
        }),
    )
}

/// Extension points allowing downstream distributions to replace selected
/// handlers while reusing the rest of the router unchanged.
#[derive(Default)]
pub struct RouterHooks {
    /// Replaces the default POST /api/sessions/{id}/agent handler.
    pub start_agent: Option<axum::routing::MethodRouter<Arc<AppState>>>,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    build_router_with_hooks(state, RouterHooks::default())
}

pub fn build_router_with_hooks(state: Arc<AppState>, hooks: RouterHooks) -> Router {
    let api = Router::new()
        // Liveness/readiness for load balancers and orchestrators.
        .route("/health", get(handle_health))
        // Auth
        .route("/auth/login", post(handle_login))
        .route("/auth/admin-login", post(handle_admin_login))
        .route("/auth/check", get(handle_check))
        .route("/auth/change-password", post(handle_change_password))
        // Admin: user management
        .route("/admin/stats", get(handle_admin_stats))
        .route("/admin/users", get(handle_list_users))
        .route("/admin/users", post(handle_create_user))
        .route("/admin/users/{id}", delete(handle_delete_user))
        .route(
            "/admin/users/{id}/reset-password",
            post(handle_reset_user_password),
        )
        .route("/admin/terminal", post(handle_create_admin_terminal))
        // Servers
        .route("/groups", get(handle_list_groups))
        .route("/servers", get(handle_list_servers))
        .route("/servers", post(handle_create_server))
        .route("/servers/{id}", put(handle_update_server))
        .route("/servers/{id}", delete(handle_delete_server))
        .route("/servers/{id}/test", post(handle_test_server))
        // Keys
        .route("/keys", get(handle_list_keys))
        .route("/keys", post(handle_create_key))
        .route("/keys/{id}", delete(handle_delete_key))
        // Sessions
        .route("/sessions", get(handle_list_sessions))
        .route("/sessions", post(handle_create_session))
        .route("/sessions/{id}", delete(handle_delete_session))
        // Files (SFTP)
        .route("/sessions/{id}/files", get(handle_list_files))
        .route("/sessions/{id}/files/content", get(handle_read_file))
        .route(
            "/sessions/{id}/files/content",
            put(handle_write_file).layer(DefaultBodyLimit::max(limits::MAX_FILE_WRITE_BYTES)),
        )
        .route("/sessions/{id}/files", delete(handle_delete_file))
        .route("/sessions/{id}/files/mkdir", post(handle_mkdir))
        .route("/sessions/{id}/files/rename", post(handle_rename_file))
        .route("/sessions/{id}/files/home", get(handle_home_dir))
        .route("/sessions/{id}/files/stat", get(handle_stat_file))
        .route("/sessions/{id}/files/download", get(handle_download_file))
        .route(
            "/sessions/{id}/files/upload",
            post(handle_upload_file).layer(DefaultBodyLimit::max(limits::MAX_UPLOAD_BYTES)),
        )
        // Git
        .route("/sessions/{id}/git/status", get(handle_git_status))
        .route("/sessions/{id}/git/log", get(handle_git_log))
        .route("/sessions/{id}/git/branches", get(handle_git_branches))
        .route("/sessions/{id}/git/diff", get(handle_git_diff))
        // Agent
        .route(
            "/sessions/{id}/agent",
            hooks
                .start_agent
                .unwrap_or_else(|| post(handle_start_agent)),
        )
        .route("/sessions/{id}/agent", delete(handle_stop_agent))
        .route("/sessions/{id}/agent/status", get(handle_agent_status))
        // AI Tools (admin)
        .route("/admin/tools", get(handle_list_tools))
        .route("/admin/tools", post(handle_create_tool))
        .route("/admin/tools/{id}", put(handle_update_tool))
        .route("/admin/tools/{id}", delete(handle_delete_tool))
        // AI Tools (user config)
        .route("/tools", get(handle_list_tools_for_user))
        .route("/tools/{id}/config", get(handle_get_tool_config))
        .route("/tools/{id}/config", put(handle_save_tool_config))
        .route("/tools/{id}/config", delete(handle_delete_tool_config))
        // AI Tools (server-level config)
        .route(
            "/tools/{id}/server-config",
            get(handle_get_server_tool_config),
        )
        .route(
            "/tools/{id}/server-config",
            put(handle_save_server_tool_config),
        )
        .route(
            "/tools/{id}/server-config",
            delete(handle_delete_server_tool_config),
        )
        // Audit
        .route("/audit/logs", get(handle_list_audit_logs))
        .route("/audit/filters", get(handle_audit_filters))
        // UI State
        .route("/ui-state", get(handle_get_ui_state))
        .route("/ui-state", put(handle_save_ui_state));

    let router = Router::new()
        .nest("/api", api)
        .route("/ws/terminal/{session_id}", get(ws_handler::ws_terminal))
        .route("/ws/agent/{agent_id}", get(ws_handler::ws_agent))
        .fallback(static_handler)
        .layer(DefaultBodyLimit::max(limits::MAX_JSON_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    #[cfg(debug_assertions)]
    let router = router.layer(CorsLayer::permissive());

    router
}

/// Unauthenticated health check. Reports whether the database answers and
/// how much is live; returns 503 when the database is unusable so an
/// orchestrator stops routing traffic here.
async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_ok = state
        .db
        .conn()
        .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
        .map(|v| v == 1)
        .unwrap_or(false);
    let body = serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "db": if db_ok { "ok" } else { "error" },
        "activeSessions": state.sessions.list_all_session_count(),
        "activeAgents": state.agents.active_count().await,
        "schemaVersion": state.db.schema_version(),
    });
    let code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for client-side routing
            match Assets::get("index.html") {
                Some(index) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html".to_string())],
                    index.data.to_vec(),
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}

// --- Auth handlers ---

async fn handle_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<auth::LoginRequest>,
) -> impl IntoResponse {
    let ip = addr.ip();
    if !state.rate_limiter.check(ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                error: "too many login attempts, try again later".to_string(),
            }),
        )
            .into_response();
    }

    let login_state = state.clone();
    let (username, password) = (req.username.clone(), req.password.clone());
    let result = auth::run_blocking(move || {
        auth::login(
            &login_state.db,
            &username,
            &password,
            &login_state.config.pepper,
            &login_state.config.jwt_secret,
        )
    })
    .await;
    match result {
        Ok((token, user_id, user_secret)) => {
            state.rate_limiter.reset(ip);
            state
                .auth_sessions
                .write()
                .await
                .insert(user_id.clone(), user_secret);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "token": token,
                    "userId": user_id,
                    "username": req.username,
                    "role": "user",
                })),
            )
                .into_response()
        }
        Err(e) => {
            state.rate_limiter.record_failure(ip);
            (StatusCode::UNAUTHORIZED, Json(ApiError { error: e })).into_response()
        }
    }
}

async fn handle_admin_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<auth::AdminLoginRequest>,
) -> impl IntoResponse {
    let ip = addr.ip();
    if !state.rate_limiter.check(ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                error: "too many login attempts, try again later".to_string(),
            }),
        )
            .into_response();
    }

    if auth::verify_admin(&state.config.data_dir, &req.master_key) {
        state.rate_limiter.reset(ip);
        let token =
            auth::create_token("admin", "admin", &state.config.jwt_secret).unwrap_or_default();

        // Derive a stable admin secret from pepper so admin can use tool config encryption
        let admin_secret = crate::crypto::derive_data_key(
            &crate::crypto::derive_data_key(
                &[0u8; 32], // fixed salt for admin
                "admin-session-secret",
            ),
            &state.config.pepper,
        );
        state
            .auth_sessions
            .write()
            .await
            .insert("admin".to_string(), admin_secret);

        (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "userId": "admin",
                "username": "admin",
                "role": "admin",
            })),
        )
            .into_response()
    } else {
        state.rate_limiter.record_failure(ip);
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                error: "Invalid master key".into(),
            }),
        )
            .into_response()
    }
}

async fn handle_check(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = extract_token(&headers);
    match token.and_then(|t| auth::verify_token(&t, &state.config.jwt_secret)) {
        Some((user_id, role)) => {
            if role != "admin"
                && state
                    .auth_sessions
                    .read()
                    .await
                    .get_user_secret(&user_id)
                    .is_none()
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({"userId": user_id, "role": role})),
            )
                .into_response()
        }
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

async fn handle_change_password(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let cp_state = state.clone();
    let uid = user_id.clone();
    let result = auth::run_blocking(move || {
        auth::change_password(
            &cp_state.db,
            &uid,
            &req.old_password,
            &req.new_password,
            &cp_state.config.pepper,
        )
    })
    .await;
    match result {
        Ok(()) => {
            state.auth_sessions.write().await.remove(&user_id);
            Json(serde_json::json!({"message": "Password changed. Please login again."}))
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

// --- Admin handlers ---

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    password: Option<String>,
}

async fn handle_admin_stats(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    let active_sessions = state.sessions.list_all_session_count();
    Json(serde_json::json!({ "activeSessions": active_sessions })).into_response()
}

async fn handle_list_users(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match auth::list_users(&state.db) {
        Ok(users) => Json(users).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_create_user(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let password = req.password.unwrap_or_else(|| {
        let bytes: [u8; 8] = crypto::generate_random_bytes();
        hex::encode(bytes)
    });
    let cu_state = state.clone();
    let username = req.username.clone();
    let result = auth::run_blocking(move || {
        auth::create_user(&cu_state.db, &username, &password, &cu_state.config.pepper)
    })
    .await;
    match result {
        Ok((user_id, pwd)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"userId": user_id, "username": req.username, "password": pwd})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_delete_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match auth::delete_user(&state.db, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetPasswordRequest {
    new_password: String,
}

async fn handle_reset_user_password(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    Json(req): Json<ResetPasswordRequest>,
) -> impl IntoResponse {
    let rp_state = state.clone();
    let uid = id.clone();
    let result = auth::run_blocking(move || {
        auth::admin_reset_password(
            &rp_state.db,
            &uid,
            &req.new_password,
            &rp_state.config.pepper,
        )
    })
    .await;
    match result {
        Ok(()) => {
            state.auth_sessions.write().await.remove(&id);
            Json(serde_json::json!({"message": "Password reset. User's SSH keys have been cleared."}))
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

// --- Admin terminal handler ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAdminTerminalRequest {
    cols: Option<u32>,
    rows: Option<u32>,
    ai_tool_id: Option<String>,
    parent_session_id: Option<String>,
}

async fn handle_create_admin_terminal(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    _admin: AdminUser,
    Json(req): Json<CreateAdminTerminalRequest>,
) -> impl IntoResponse {
    // For root sessions (no parent), serialize to prevent duplicate creation
    if req.parent_session_id.is_none() {
        let _guard = state.admin_terminal_lock.lock().await;
        let existing = state.sessions.list_sessions("admin");
        if let Some(root) = existing.iter().find(|s| s.parent_session_id.is_none()) {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": root.id,
                    "serverAlias": "localhost",
                    "serverHost": "localhost",
                    "serverType": "local",
                    "aiToolId": root.ai_tool_id,
                })),
            )
                .into_response();
        }
    }

    let cols = req.cols.unwrap_or(state.config.default_cols as u32) as u16;
    let rows = req.rows.unwrap_or(state.config.default_rows as u32) as u16;

    let pty_session = match crate::local_pty::spawn(cols, rows) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };

    let session_id = state.sessions.create_session(
        "local".to_string(),
        "localhost".to_string(),
        "localhost".to_string(),
        req.ai_tool_id.clone(),
        req.parent_session_id.clone(),
        "admin".to_string(),
        pty_session.output_rx,
        pty_session.cmd_tx,
        3600,
        (cols, rows),
    );

    state.helpers.register_local(&session_id).await;
    if !state.sessions.session_exists(&session_id) {
        state.helpers.remove(&session_id).await;
    }
    if let Err(e) = audit_events::session_started(
        &state.db,
        &audit_events::AuditSession {
            session_id: session_id.clone(),
            actor: audit_events::Actor {
                kind: Some(audit_events::ActorKind::Human),
                user_id: Some("admin".into()),
                username: Some("admin".into()),
                remote_addr: Some(addr.ip().to_string()),
                ..Default::default()
            },
            target: audit_events::Target {
                server_id: Some("local".into()),
                server_alias: Some("localhost".into()),
                server_host: Some("localhost".into()),
                remote_user: None,
            },
            source: audit_events::Source::Terminal,
            parent_session_id: req.parent_session_id.clone(),
            connected_at: chrono::Utc::now().to_rfc3339(),
            disconnected_at: None,
            disconnect_reason: None,
            integrity: audit_events::Integrity::Complete,
        },
    ) {
        tracing::error!("Failed to record audit session: {}", e);
    }

    let _ = audit::log_connect(
        &state.db,
        "admin",
        "admin",
        "local",
        "localhost",
        "localhost",
        &session_id,
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": session_id,
            "serverAlias": "localhost",
            "serverHost": "localhost",
            "serverType": "local",
            "aiToolId": req.ai_tool_id,
        })),
    )
        .into_response()
}

// --- Server handlers ---

async fn handle_list_groups(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match server_registry::list_groups(&state.db) {
        Ok(groups) => Json(groups).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ServerQuery {
    group_name: Option<String>,
}

async fn handle_list_servers(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ServerQuery>,
) -> impl IntoResponse {
    match server_registry::list_servers(&state.db, q.group_name.as_deref()) {
        Ok(servers) => Json(servers).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_create_server(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateServerRequest>,
) -> impl IntoResponse {
    match server_registry::create_server(&state.db, &req) {
        Ok(server) => (StatusCode::CREATED, Json(server)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_delete_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match server_registry::delete_server(&state.db, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_update_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<server_registry::UpdateServerRequest>,
) -> impl IntoResponse {
    match server_registry::update_server(&state.db, &id, &req) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_test_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(key) => key,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let server = match server_registry::get_server(&state.db, &id) {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };

    let key_id = match &server.key_id {
        Some(kid) => kid.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: "No key assigned".into(),
                }),
            )
                .into_response()
        }
    };

    let private_key_pem = match key_manager::decrypt_key(&state.db, &user_id, &user_secret, &key_id)
    {
        Ok(pem) => pem,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };

    match ssh_bridge::connect(
        &server.host,
        server.port,
        &server.username,
        &private_key_pem,
        80,
        24,
        server.host_key_fingerprint.clone(),
    )
    .await
    {
        Ok(session) => {
            let _ = session.cmd_tx.send(ssh_bridge::SshCommand::Close).await;
            Json(serde_json::json!({"success": true, "message": "Connection successful"}))
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"success": false, "message": e})),
        )
            .into_response(),
    }
}

// --- Key handlers ---

async fn handle_list_keys(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    match key_manager::list_keys(&state.db, &user_id) {
        Ok(keys) => Json(keys).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_create_key(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(key) => key,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match key_manager::create_key(
        &state.db,
        &user_id,
        &user_secret,
        &req.name,
        &req.private_key,
    ) {
        Ok(info) => {
            tracing::info!(
                "Key created: id={}, pem_len={}, first_line='{}'",
                info.id,
                req.private_key.len(),
                req.private_key.lines().next().unwrap_or("")
            );
            (StatusCode::CREATED, Json(info)).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    match key_manager::delete_key(&state.db, &user_id, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

// --- Session handlers ---

async fn handle_list_sessions(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let sessions = state.sessions.list_sessions(&user_id);
    Json(sessions).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    server_id: String,
    cols: Option<u32>,
    rows: Option<u32>,
    parent_session_id: Option<String>,
}

async fn handle_create_session(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(key) => key,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if state.sessions.session_count_for_user(&user_id) >= limits::MAX_SESSIONS_PER_USER {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ApiError {
                error: format!(
                    "Too many open sessions (limit {})",
                    limits::MAX_SESSIONS_PER_USER
                ),
            }),
        )
            .into_response();
    }

    let server = match server_registry::get_server(&state.db, &req.server_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Server not found".into(),
                }),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };

    let key_id = match &server.key_id {
        Some(kid) => kid.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: "No key assigned to server".into(),
                }),
            )
                .into_response()
        }
    };

    tracing::info!(
        "Connecting to {}@{}:{} with key_id={}",
        server.username,
        server.host,
        server.port,
        key_id
    );

    let private_key_pem = match key_manager::decrypt_key(&state.db, &user_id, &user_secret, &key_id)
    {
        Ok(pem) => {
            tracing::info!("Key decrypted successfully, PEM length={}", pem.len());
            pem
        }
        Err(e) => {
            tracing::error!("Key decryption failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response();
        }
    };

    let cols = req
        .cols
        .unwrap_or(state.config.default_cols as u32)
        .clamp(1, 500);
    let rows = req
        .rows
        .unwrap_or(state.config.default_rows as u32)
        .clamp(1, 200);

    let ssh_session = match ssh_bridge::connect(
        &server.host,
        server.port,
        &server.username,
        &private_key_pem,
        cols,
        rows,
        server.host_key_fingerprint.clone(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(ApiError { error: e })).into_response(),
    };

    // Store host key fingerprint on first connection (TOFU)
    if server.host_key_fingerprint.is_none() {
        let update = crate::server_registry::UpdateServerRequest {
            group_name: None,
            alias: None,
            host: None,
            port: None,
            username: None,
            key_id: None,
            tags: None,
            ai_tool_id: None,
            host_key_fingerprint: Some(ssh_session.host_key_fingerprint.clone()),
            idle_timeout_secs: None,
        };
        let _ = crate::server_registry::update_server(&state.db, &server.id, &update);
    }

    let session_id = state.sessions.create_session(
        server.id.clone(),
        server.alias.clone(),
        server.host.clone(),
        server.ai_tool_id.clone(),
        req.parent_session_id.clone(),
        user_id.clone(),
        ssh_session.output_rx,
        ssh_session.cmd_tx,
        server.idle_timeout_secs,
        (cols as u16, rows as u16),
    );

    state
        .helpers
        .register_handle(&session_id, ssh_session.handle)
        .await;
    // The transport may already have died and been reaped before the helper
    // was registered; do not leave an orphaned connection behind.
    if !state.sessions.session_exists(&session_id) {
        state.helpers.remove(&session_id).await;
    }

    let username = state
        .db
        .conn()
        .query_row(
            "SELECT username FROM users WHERE id = ?1",
            rusqlite::params![user_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default();

    let _ = audit::log_connect(
        &state.db,
        &user_id,
        &username,
        &server.id,
        &server.alias,
        &server.host,
        &session_id,
    );
    if let Err(e) = audit_events::session_started(
        &state.db,
        &audit_events::AuditSession {
            session_id: session_id.clone(),
            actor: audit_events::Actor {
                kind: Some(audit_events::ActorKind::Human),
                user_id: Some(user_id.clone()),
                username: Some(username.clone()),
                remote_addr: Some(addr.ip().to_string()),
                ..Default::default()
            },
            target: audit_events::Target {
                server_id: Some(server.id.clone()),
                server_alias: Some(server.alias.clone()),
                server_host: Some(server.host.clone()),
                remote_user: Some(server.username.clone()),
            },
            source: audit_events::Source::Terminal,
            parent_session_id: req.parent_session_id.clone(),
            connected_at: chrono::Utc::now().to_rfc3339(),
            disconnected_at: None,
            disconnect_reason: None,
            integrity: audit_events::Integrity::Complete,
        },
    ) {
        tracing::error!("Failed to record audit session: {}", e);
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": session_id,
            "serverId": server.id,
            "serverAlias": server.alias,
            "serverHost": server.host,
            "aiToolId": server.ai_tool_id,
        })),
    )
        .into_response()
}

async fn handle_delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    // Closing goes through the session's fan-out task; the helper
    // connection, bound agent and audit row are released by the reaper when
    // it reports SessionEnded, exactly as for any other exit.
    if state.sessions.remove_session(&id, &user_id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

// --- Audit handlers ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    limit: Option<u32>,
    offset: Option<u32>,
    server: Option<String>,
    status: Option<String>,
    time_from: Option<String>,
    time_to: Option<String>,
    username: Option<String>,
}

async fn handle_list_audit_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let caller_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let is_admin = is_admin(&headers, &state.config.jwt_secret);
    let limit = q.limit.unwrap_or(50).min(200);
    let offset = q.offset.unwrap_or(0);

    let filter = audit::AuditFilter {
        user_id: if is_admin { None } else { Some(caller_id) },
        username: if is_admin { q.username } else { None },
        server: q.server,
        status: q.status,
        time_from: q.time_from,
        time_to: q.time_to,
    };

    match audit::list_logs(&state.db, &filter, limit, offset) {
        Ok((items, total)) => {
            Json(serde_json::json!({ "items": items, "total": total })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_audit_filters(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let caller_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let is_admin = is_admin(&headers, &state.config.jwt_secret);
    let user_id_filter = if is_admin {
        None
    } else {
        Some(caller_id.as_str())
    };

    match audit::list_filter_options(&state.db, user_id_filter) {
        Ok((usernames, servers)) => {
            Json(serde_json::json!({ "usernames": usernames, "servers": servers })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

// --- Helpers ---

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

fn extract_user_id(headers: &axum::http::HeaderMap, jwt_secret: &[u8]) -> Option<String> {
    extract_token(headers).and_then(|t| auth::verify_token(&t, jwt_secret).map(|(uid, _)| uid))
}

fn is_admin(headers: &axum::http::HeaderMap, jwt_secret: &[u8]) -> bool {
    extract_token(headers)
        .and_then(|t| auth::verify_token(&t, jwt_secret))
        .map(|(_, role)| role == "admin")
        .unwrap_or(false)
}

// --- File browser handlers ---

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteFileRequest {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MkdirRequest {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameRequest {
    from: String,
    to: String,
}

async fn ensure_helper(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let server_id = state.sessions.get_server_id(session_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "Session not found".into(),
            }),
        )
    })?;

    if server_id == "local" {
        state.helpers.register_local(session_id).await;
        return Ok(());
    }

    let server = server_registry::get_server(&state.db, &server_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Server not found".into(),
                }),
            )
        })?;

    let key_id = server.key_id.as_ref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "No key assigned".into(),
            }),
        )
    })?;

    let user_secret = state
        .auth_sessions
        .read()
        .await
        .get_user_secret(user_id)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ApiError {
                    error: "Session expired".into(),
                }),
            )
        })?;

    let private_key_pem = key_manager::decrypt_key(&state.db, user_id, &user_secret, key_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
        })?;

    let info = crate::helper_pool::ConnectInfo {
        host: server.host,
        port: server.port,
        username: server.username,
        private_key_pem,
    };

    state
        .helpers
        .get_or_connect(session_id, &info)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
        })
}

async fn handle_list_files(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&query.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state.helpers.sftp_list(&session_id, &query.path).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_read_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&query.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    // Preview loads the whole file into memory: refuse oversized files up
    // front instead of buffering them.
    if let Ok(meta) = state.helpers.sftp_stat(&session_id, &query.path).await {
        if meta.size > limits::MAX_PREVIEW_BYTES {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ApiError {
                    error: format!(
                        "File is {} bytes; preview is limited to {} bytes. Use download instead.",
                        meta.size,
                        limits::MAX_PREVIEW_BYTES
                    ),
                }),
            )
                .into_response();
        }
    }

    match state.helpers.sftp_read(&session_id, &query.path).await {
        Ok(data) => {
            if data.len() as u64 > limits::MAX_PREVIEW_BYTES {
                return (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(ApiError {
                        error: "File too large to preview".to_string(),
                    }),
                )
                    .into_response();
            }
            let content = String::from_utf8_lossy(&data).to_string();
            Json(serde_json::json!({ "content": content })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_write_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WriteFileRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&req.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state
        .helpers
        .sftp_write(&session_id, &req.path, req.content.as_bytes())
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_delete_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&query.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state.helpers.sftp_delete(&session_id, &query.path).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_mkdir(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<MkdirRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&req.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state.helpers.sftp_mkdir(&session_id, &req.path).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_rename_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RenameRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&req.from) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }
    if let Err(e) = crate::utils::validate_path(&req.to) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state
        .helpers
        .sftp_rename(&session_id, &req.from, &req.to)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_home_dir(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    match state.helpers.sftp_realpath(&session_id, ".").await {
        Ok(path) => Json(serde_json::json!({ "path": path })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_stat_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&query.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    match state.helpers.sftp_stat(&session_id, &query.path).await {
        Ok(entry) => Json(entry).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_download_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    if let Err(e) = crate::utils::validate_path(&query.path) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    let file_size = state
        .helpers
        .sftp_stat(&session_id, &query.path)
        .await
        .map(|s| s.size)
        .unwrap_or(0);

    let file = match state.helpers.open_read(&session_id, &query.path).await {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };

    let filename = std::path::Path::new(&query.path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let stream = tokio_util::io::ReaderStream::with_capacity(file, 65536);
    let body = axum::body::Body::from_stream(stream);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .unwrap(),
    );
    if file_size > 0 {
        headers.insert(
            header::CONTENT_LENGTH,
            file_size.to_string().parse().unwrap(),
        );
    }

    (headers, body).into_response()
}

async fn handle_upload_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<FileQuery>,
    AuthUser(user_id): AuthUser,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    let upload_dir = &query.path;
    if let Err(e) = crate::utils::validate_path(upload_dir) {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
    }

    let mut uploaded = Vec::new();
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("file").to_string();
        if name == "path" {
            continue;
        }
        let file_name = field.file_name().unwrap_or("unnamed").to_string();
        let remote_path = format!("{}/{}", upload_dir.trim_end_matches('/'), file_name);

        let mut file = match state.helpers.open_write(&session_id, &remote_path).await {
            Ok(f) => f,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: e }),
                )
                    .into_response();
            }
        };

        let mut total_written: u64 = 0;
        let mut write_err = None;
        while let Ok(Some(chunk)) = field.chunk().await {
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
                write_err = Some(format!("Write failed: {}", e));
                break;
            }
            total_written += chunk.len() as u64;
        }

        if let Err(e) = tokio::io::AsyncWriteExt::flush(&mut file).await {
            if write_err.is_none() {
                write_err = Some(format!("Flush failed: {}", e));
            }
        }

        if let Some(e) = write_err {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response();
        }

        uploaded.push(serde_json::json!({
            "name": file_name,
            "path": remote_path,
            "size": total_written,
        }));
    }

    Json(serde_json::json!({ "uploaded": uploaded })).into_response()
}

// --- Git handlers ---

#[derive(Deserialize)]
struct GitLogQuery {
    limit: Option<u32>,
}

async fn handle_git_status(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    let cmd = "git status --porcelain 2>/dev/null && echo '---BRANCH---' && git branch --show-current 2>/dev/null";
    match state.helpers.exec(&session_id, cmd).await {
        Ok(output) => {
            let parts: Vec<&str> = output.splitn(2, "---BRANCH---").collect();
            let files: Vec<serde_json::Value> = parts
                .first()
                .unwrap_or(&"")
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let status = &l[..2];
                    let file = l[3..].to_string();
                    serde_json::json!({ "status": status.trim(), "file": file })
                })
                .collect();
            let branch = parts.get(1).unwrap_or(&"").trim().to_string();
            Json(serde_json::json!({ "branch": branch, "files": files })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_git_log(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<GitLogQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    let limit = query.limit.unwrap_or(50);
    let cmd = format!(
        "git log --oneline --decorate --format='%H|||%h|||%s|||%an|||%ar|||%D' -n {} 2>/dev/null",
        limit
    );
    match state.helpers.exec(&session_id, &cmd).await {
        Ok(output) => {
            let commits: Vec<serde_json::Value> = output
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let parts: Vec<&str> = l.splitn(6, "|||").collect();
                    serde_json::json!({
                        "hash": parts.first().unwrap_or(&""),
                        "shortHash": parts.get(1).unwrap_or(&""),
                        "message": parts.get(2).unwrap_or(&""),
                        "author": parts.get(3).unwrap_or(&""),
                        "relativeDate": parts.get(4).unwrap_or(&""),
                        "refs": parts.get(5).unwrap_or(&""),
                    })
                })
                .collect();
            Json(commits).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_git_branches(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    let cmd = "git branch -a --format='%(refname:short)|||%(HEAD)|||%(upstream:short)' 2>/dev/null";
    match state.helpers.exec(&session_id, cmd).await {
        Ok(output) => {
            let branches: Vec<serde_json::Value> = output
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let parts: Vec<&str> = l.splitn(3, "|||").collect();
                    let name = parts.first().unwrap_or(&"").to_string();
                    let is_current = parts.get(1).unwrap_or(&"") == &"*";
                    let upstream = parts.get(2).unwrap_or(&"").to_string();
                    serde_json::json!({
                        "name": name,
                        "current": is_current,
                        "upstream": upstream,
                    })
                })
                .collect();
            Json(branches).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_git_diff(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }

    let cmd = "git diff 2>/dev/null";
    match state.helpers.exec(&session_id, cmd).await {
        Ok(diff) => Json(serde_json::json!({ "diff": diff })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

// --- Agent handlers ---

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAgentRequest {
    pub prompt: String,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// User-chosen approval level: `none`, `low`, `medium`, `high`,
    /// `critical` or `all`. It can only tighten the administrator's policy.
    #[serde(default)]
    pub approval_level: Option<String>,
}

/// Everything resolved from the request before an agent is launched:
/// ownership-checked identity, the tool definition, and the merged
/// (admin defaults + user + server overrides) parameter values.
pub struct AgentLaunchContext {
    pub user_id: String,
    pub server_id: String,
    pub tool: crate::ai_tool_registry::AiTool,
    pub params: Vec<crate::ai_tool_registry::ParamDef>,
    pub param_values: std::collections::HashMap<String, String>,
    pub exec_opts: crate::ai_tool_registry::ExecutionOptions,
}

/// Auth + tool loading + three-level param merge shared by all agent
/// start handlers. Returns a ready-to-use response on failure.
pub async fn resolve_agent_launch(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    session_id: &str,
    tool_id: &str,
) -> Result<AgentLaunchContext, axum::response::Response> {
    let user_id = match extract_user_id(headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return Err(StatusCode::UNAUTHORIZED.into_response()),
    };
    if !state.sessions.is_owner(session_id, &user_id) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }

    let server_id = match state.sessions.get_server_id(session_id) {
        Some(id) => id,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Session not found".into(),
                }),
            )
                .into_response())
        }
    };

    // Get user_secret for tool config (needed for env var decryption)
    let user_secret_opt = state.auth_sessions.read().await.get_user_secret(&user_id);

    let tool = match crate::ai_tool_registry::get_tool(&state.db, tool_id) {
        Ok(Some(t)) => t,
        _ => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "Tool not found".into(),
                }),
            )
                .into_response())
        }
    };

    // Load user param values (decrypted from user_tool_configs)
    let mut param_values = if let Some(ref us) = user_secret_opt {
        crate::user_tool_config::get_decrypted_env(&state.db, &user_id, us, tool_id)
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    // Fill in tool definition defaults for params not already set
    let params = tool.params();
    for param in &params {
        if !param_values.contains_key(&param.key) {
            if let Some(ref default_val) = param.default {
                if !default_val.is_empty() {
                    param_values.insert(param.key.clone(), default_val.clone());
                }
            }
        }
    }

    // Apply per-user disabled keys
    if let Ok(Some(ref user_cfg)) =
        crate::user_tool_config::get_config(&state.db, &user_id, tool_id)
    {
        for key in &user_cfg.disabled_keys {
            param_values.remove(key);
        }
    }

    // Apply per-server overrides
    if let (Some(ref us), Ok(Some(server_cfg))) = (
        &user_secret_opt,
        crate::server_tool_config::get_config(&state.db, &user_id, &server_id, tool_id),
    ) {
        for key in &server_cfg.disabled_keys {
            param_values.remove(key);
        }
        if let Ok(overrides) = crate::server_tool_config::get_decrypted_env(
            &state.db, &user_id, us, &server_id, tool_id,
        ) {
            param_values.extend(overrides);
        }
    }

    let exec_opts = match tool.execution_options() {
        Ok(opts) => opts,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: format!("Tool policy configuration is invalid: {}", e),
                }),
            )
                .into_response())
        }
    };

    Ok(AgentLaunchContext {
        user_id,
        server_id,
        tool,
        params,
        param_values,
        exec_opts,
    })
}

/// Launch an external CLI agent tool (e.g. Claude Code) for the session.
pub async fn start_agent_inner(
    state: &Arc<AppState>,
    session_id: &str,
    ctx: &AgentLaunchContext,
    body: &StartAgentRequest,
) -> axum::response::Response {
    let tool_id = &ctx.tool.id;
    let ext = match ctx.tool.external_options() {
        Ok(ext) => ext,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: format!("Tool configuration is invalid: {}", e),
                }),
            )
                .into_response()
        }
    };
    if ext.launch_cmd.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "Tool has no launch command configured".into(),
            }),
        )
            .into_response();
    }

    // External CLIs run their own permission model. Only tools that declare
    // `supports_approval` get platform-driven flags; for the rest the UI must
    // not promise per-operation approval and we pass the command unchanged.
    let mut launch_cmd = ext.launch_cmd.clone();
    if ext.supports_approval && !body.require_approval {
        launch_cmd.push_str(" --permission-mode acceptEdits");
    }

    // Collect usage=env params as environment variables
    let env_vars: std::collections::HashMap<String, String> = ctx
        .params
        .iter()
        .filter(|p| p.usage == "env")
        .filter_map(|p| {
            ctx.param_values
                .get(&p.key)
                .map(|v| (p.key.clone(), v.clone()))
        })
        .collect();

    // Build config from config_tpl + user/server overrides
    let user_config_override =
        crate::user_tool_config::get_config(&state.db, &ctx.user_id, tool_id)
            .ok()
            .flatten()
            .and_then(|c| c.config_override);

    let server_config_override =
        crate::server_tool_config::get_config(&state.db, &ctx.user_id, &ctx.server_id, tool_id)
            .ok()
            .flatten()
            .and_then(|c| c.config_override);

    let config_tpl = ext.config_tpl.as_deref().unwrap_or("{}");
    let config =
        if user_config_override.is_some() || server_config_override.is_some() || config_tpl != "{}"
        {
            let merged = merge_json_config(config_tpl, user_config_override.as_deref());
            Some(merge_json_config(
                &merged,
                server_config_override.as_deref(),
            ))
        } else {
            None
        };

    let tool_context = crate::agent_bridge::AgentToolContext {
        detect_cmd: ext.detect_cmd,
        install_cmd: ext.install_cmd,
        config,
        config_path: ext.config_path,
        launch_cmd,
        env_vars,
    };

    let agent_id = format!("agent-{}", session_id);

    match state
        .agents
        .start(
            &agent_id,
            session_id,
            &ctx.user_id,
            &state.helpers,
            &tool_context,
            &body.prompt,
            body.working_dir.as_deref(),
        )
        .await
    {
        Ok(()) => Json(serde_json::json!({
            "agentId": agent_id,
            "supportsApproval": ext.supports_approval,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("Agent start failed [{}]: {}", agent_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    }
}

async fn handle_start_agent(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<StartAgentRequest>,
) -> impl IntoResponse {
    let Some(ref tool_id) = body.tool_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "tool_id is required".into(),
            }),
        )
            .into_response();
    };

    let ctx = match resolve_agent_launch(&state, &headers, &session_id, tool_id).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    if ctx.tool.tool_type == "native" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "Native engine tools require NGTerm EE".into(),
            }),
        )
            .into_response();
    }

    start_agent_inner(&state, &session_id, &ctx, &body).await
}

fn merge_json_config(base: &str, override_json: Option<&str>) -> String {
    let mut base_val: serde_json::Value =
        serde_json::from_str(base).unwrap_or(serde_json::Value::Object(Default::default()));
    if let Some(ovr) = override_json {
        if let Ok(ovr_val) = serde_json::from_str::<serde_json::Value>(ovr) {
            if let (Some(b), Some(o)) = (base_val.as_object_mut(), ovr_val.as_object()) {
                for (k, v) in o {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
    }
    serde_json::to_string_pretty(&base_val).unwrap_or_else(|_| "{}".to_string())
}

async fn handle_stop_agent(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent_id = format!("agent-{}", session_id);
    state.agents.stop(&agent_id).await;
    state.helpers.remove_user(&session_id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn handle_agent_status(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let agent_id = format!("agent-{}", session_id);
    let active = state.agents.is_active(&agent_id).await;
    Json(serde_json::json!({ "active": active, "agentId": agent_id })).into_response()
}

// --- AI Tool handlers (admin) ---

async fn handle_list_tools(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match crate::ai_tool_registry::list_tools(&state.db) {
        Ok(tools) => Json(tools).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_create_tool(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(req): Json<crate::ai_tool_registry::CreateAiToolRequest>,
) -> impl IntoResponse {
    match crate::ai_tool_registry::create_tool(&state.db, &req) {
        Ok(tool) => (StatusCode::CREATED, Json(tool)).into_response(),
        Err(e) => registry_error_response(e),
    }
}

/// Map registry errors to HTTP: validation problems carry field-level
/// details so the admin form can highlight the offending inputs.
fn registry_error_response(e: crate::ai_tool_registry::RegistryError) -> axum::response::Response {
    use crate::ai_tool_registry::RegistryError;
    match e {
        RegistryError::Validation(fields) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": RegistryError::Validation(fields.clone()).to_string(),
                "fields": fields,
            })),
        )
            .into_response(),
        RegistryError::NotFound => StatusCode::NOT_FOUND.into_response(),
        RegistryError::Db(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: msg }),
        )
            .into_response(),
    }
}

async fn handle_update_tool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    Json(req): Json<crate::ai_tool_registry::UpdateAiToolRequest>,
) -> impl IntoResponse {
    match crate::ai_tool_registry::update_tool(&state.db, &id, &req) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => registry_error_response(e),
    }
}

async fn handle_delete_tool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
) -> impl IntoResponse {
    match crate::ai_tool_registry::delete_tool(&state.db, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

// --- AI Tool config handlers (user) ---

async fn handle_list_tools_for_user(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = state.auth_sessions.read().await.get_user_secret(&user_id);

    let tools = match crate::ai_tool_registry::list_tools(&state.db) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };
    let configs = crate::user_tool_config::list_configs(&state.db, &user_id).unwrap_or_default();
    let configured_tool_ids: std::collections::HashSet<String> =
        configs.iter().map(|c| c.tool_id.clone()).collect();

    let result: Vec<serde_json::Value> = tools
        .into_iter()
        .map(|t| {
            let tool_id = t.id.clone();
            let configured = configured_tool_ids.contains(&tool_id);
            let mut val = serde_json::to_value(&t).unwrap_or_default();
            val.as_object_mut().unwrap().insert(
                "configured".to_string(),
                serde_json::Value::Bool(configured),
            );

            // Mask default values of secret params
            if let Some(params_arr) = val
                .get_mut("options")
                .and_then(|o| o.get_mut("params"))
                .and_then(|v| v.as_array_mut())
            {
                for param_def in params_arr.iter_mut() {
                    let is_secret = param_def
                        .get("secret")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        || param_def
                            .get("key")
                            .and_then(|v| v.as_str())
                            .is_some_and(is_secret_key_name);
                    if is_secret {
                        if let Some(obj) = param_def.as_object_mut() {
                            if obj
                                .get("default")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| !s.is_empty())
                            {
                                obj.insert(
                                    "default".to_string(),
                                    serde_json::Value::String("***".to_string()),
                                );
                            }
                        }
                    }
                }
            }

            // Include user's configured param values (secrets masked)
            if let Some(ref secret) = user_secret {
                if let Ok(env) = crate::user_tool_config::get_decrypted_env(
                    &state.db, &user_id, secret, &tool_id,
                ) {
                    let tool_params = t.params();
                    let secret_keys: std::collections::HashSet<&str> = tool_params
                        .iter()
                        .filter(|k| k.secret)
                        .map(|k| k.key.as_str())
                        .collect();
                    let masked: std::collections::HashMap<&String, String> = env
                        .iter()
                        .map(|(k, v)| {
                            let is_secret =
                                secret_keys.contains(k.as_str()) || is_secret_key_name(k);
                            let display = if is_secret {
                                "***".to_string()
                            } else {
                                v.clone()
                            };
                            (k, display)
                        })
                        .collect();
                    val.as_object_mut().unwrap().insert(
                        "configuredValues".to_string(),
                        serde_json::to_value(&masked).unwrap(),
                    );
                }
            }

            // Include user's execution preferences from config_override
            if let Some(cfg) = configs.iter().find(|c| c.tool_id == tool_id) {
                if let Some(ref override_str) = cfg.config_override {
                    if let Ok(prefs) = serde_json::from_str::<serde_json::Value>(override_str) {
                        val.as_object_mut()
                            .unwrap()
                            .insert("userPrefs".to_string(), prefs);
                    }
                }
            }

            val
        })
        .collect();

    Json(result).into_response()
}

async fn handle_get_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::user_tool_config::get_config(&state.db, &user_id, &tool_id) {
        Ok(Some(cfg)) => {
            let env_values = crate::user_tool_config::get_decrypted_env(
                &state.db,
                &user_id,
                &user_secret,
                &tool_id,
            )
            .unwrap_or_default();

            let secret_keys: std::collections::HashSet<String> =
                crate::ai_tool_registry::get_tool(&state.db, &tool_id)
                    .ok()
                    .flatten()
                    .map(|t| {
                        t.params()
                            .iter()
                            .filter(|k| k.secret)
                            .map(|k| k.key.clone())
                            .collect()
                    })
                    .unwrap_or_default();

            let masked: std::collections::HashMap<&String, String> = env_values
                .iter()
                .map(|(k, v)| {
                    let is_secret = secret_keys.contains(k) || is_secret_key_name(k);
                    let display = if is_secret {
                        "***".to_string()
                    } else {
                        v.clone()
                    };
                    (k, display)
                })
                .collect();

            let mut response = serde_json::to_value(&cfg).unwrap();
            response["envValues"] = serde_json::to_value(&masked).unwrap();
            Json(response).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

async fn handle_save_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<crate::user_tool_config::SaveToolConfigRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::user_tool_config::save_config(&state.db, &user_id, &user_secret, &tool_id, &req) {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_delete_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    match crate::user_tool_config::delete_config(&state.db, &user_id, &tool_id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfigQuery {
    server_id: String,
}

async fn handle_get_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    Query(query): Query<ServerConfigQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::server_tool_config::get_config(&state.db, &user_id, &query.server_id, &tool_id) {
        Ok(Some(cfg)) => {
            let env_overrides = crate::server_tool_config::get_decrypted_env(
                &state.db,
                &user_id,
                &user_secret,
                &query.server_id,
                &tool_id,
            )
            .unwrap_or_default();

            let secret_keys: std::collections::HashSet<String> =
                crate::ai_tool_registry::get_tool(&state.db, &tool_id)
                    .ok()
                    .flatten()
                    .map(|t| {
                        t.params()
                            .iter()
                            .filter(|k| k.secret)
                            .map(|k| k.key.clone())
                            .collect()
                    })
                    .unwrap_or_default();

            let masked: std::collections::HashMap<&String, String> = env_overrides
                .iter()
                .map(|(k, v)| {
                    let is_secret = secret_keys.contains(k) || is_secret_key_name(k);
                    let display = if is_secret {
                        "***".to_string()
                    } else {
                        v.clone()
                    };
                    (k, display)
                })
                .collect();

            let mut response = serde_json::to_value(&cfg).unwrap();
            response["envOverrides"] = serde_json::to_value(&masked).unwrap();
            Json(response).into_response()
        }
        Ok(None) => Json(serde_json::json!(null)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_save_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<crate::server_tool_config::SaveServerToolConfigRequest>,
) -> impl IntoResponse {
    let user_id = match extract_user_id(&headers, &state.config.jwt_secret) {
        Some(uid) => uid,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match crate::server_tool_config::save_config(&state.db, &user_id, &user_secret, &tool_id, &req)
    {
        Ok(cfg) => Json(cfg).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response(),
    }
}

async fn handle_delete_server_tool_config(
    State(state): State<Arc<AppState>>,
    Path(tool_id): Path<String>,
    Query(query): Query<ServerConfigQuery>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    match crate::server_tool_config::delete_config(&state.db, &user_id, &query.server_id, &tool_id)
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response(),
    }
}

// --- UI State ---

async fn handle_get_ui_state(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let conn = state.db.conn();
    match conn.query_row(
        "SELECT state_json FROM user_ui_state WHERE user_id = ?1",
        rusqlite::params![user_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(json) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(val) => Json(val).into_response(),
            Err(_) => Json(serde_json::json!({})).into_response(),
        },
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle_save_ui_state(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let json_str = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
    let now = chrono::Utc::now().to_rfc3339();
    let conn = state.db.conn();
    match conn.execute(
        "INSERT INTO user_ui_state (user_id, state_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET state_json = ?2, updated_at = ?3",
        rusqlite::params![user_id, json_str, now],
    ) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

fn is_secret_key_name(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.contains("KEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
}
