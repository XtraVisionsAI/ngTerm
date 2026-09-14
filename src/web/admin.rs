//! User administration and the admin local terminal.

use super::*;

// --- Admin handlers ---

#[derive(Deserialize)]
pub(super) struct CreateUserRequest {
    username: String,
    password: Option<String>,
}

pub(super) async fn handle_admin_stats(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> impl IntoResponse {
    let active_sessions = state.sessions.list_all_session_count();
    Json(serde_json::json!({ "activeSessions": active_sessions })).into_response()
}

pub(super) async fn handle_list_users(
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

pub(super) async fn handle_create_user(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    caller: Caller,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let password = req.password.unwrap_or_else(|| {
        let bytes: [u8; 8] = crypto::generate_random_bytes();
        hex::encode(bytes)
    });
    let cu_state = state.clone();
    let username = req.username.clone();
    let snapshot_name = req.username.clone();
    let change = Change::new(
        ObjectKind::User,
        Action::Create,
        format!("create user {}", req.username),
    );
    let result = audit_config::run(&state, &caller, change, async move {
        let (user_id, pwd) = auth::run_blocking(move || {
            auth::create_user(&cu_state.db, &username, &password, &cu_state.config.pepper)
        })
        .await?;
        let after = audit_config::user_snapshot(&serde_json::json!({
            "id": user_id, "username": snapshot_name, "role": "user"
        }));
        Ok(Applied::new((user_id.clone(), pwd))
            .object_id(user_id)
            .after(Some(after)))
    })
    .await;
    match result {
        Ok((user_id, pwd)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"userId": user_id, "username": req.username, "password": pwd})),
        )
            .into_response(),
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

/// Redacted snapshot of a user row (id, username, role), if the user exists.
pub(super) fn user_snapshot_by_id(state: &AppState, id: &str) -> Option<serde_json::Value> {
    auth::list_users(&state.db)
        .ok()?
        .into_iter()
        .find(|u| u.get("id").and_then(|v| v.as_str()) == Some(id))
        .map(|u| audit_config::user_snapshot(&u))
}

pub(super) fn username_from_snapshot(
    snapshot: &Option<serde_json::Value>,
    fallback: &str,
) -> String {
    snapshot
        .as_ref()
        .and_then(|s| s.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

pub(super) async fn handle_delete_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    caller: Caller,
) -> impl IntoResponse {
    let before = user_snapshot_by_id(&state, &id);
    let change = Change::new(
        ObjectKind::User,
        Action::Delete,
        format!("delete user {}", username_from_snapshot(&before, &id)),
    )
    .object_id(&id)
    .before(before);
    let result = audit_config::run(&state, &caller, change, async {
        match auth::delete_user(&state.db, &id) {
            Ok(true) => Ok(Applied::new(())),
            Ok(false) => Err(NOT_FOUND.to_string()),
            Err(e) => Err(e),
        }
    })
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => config_error_response(e, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResetPasswordRequest {
    new_password: String,
}

pub(super) async fn handle_reset_user_password(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    _admin: AdminUser,
    caller: Caller,
    Json(req): Json<ResetPasswordRequest>,
) -> impl IntoResponse {
    let rp_state = state.clone();
    let uid = id.clone();
    let before = user_snapshot_by_id(&state, &id);
    let change = Change::new(
        ObjectKind::User,
        Action::PasswordReset,
        format!(
            "reset password of user {}",
            username_from_snapshot(&before, &id)
        ),
    )
    .object_id(&id);
    let result = audit_config::run(&state, &caller, change, async move {
        auth::run_blocking(move || {
            auth::admin_reset_password(
                &rp_state.db,
                &uid,
                &req.new_password,
                &rp_state.config.pepper,
            )
        })
        .await?;
        // The password itself is never part of the record; the side effect is.
        Ok(Applied::new(()).after(Some(serde_json::json!({ "sshKeysCleared": true }))))
    })
    .await;
    match result {
        Ok(()) => {
            state.auth_sessions.write().await.remove(&id);
            Json(serde_json::json!({"message": "Password reset. User's SSH keys have been cleared."}))
                .into_response()
        }
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

// --- Admin terminal handler ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateAdminTerminalRequest {
    cols: Option<u32>,
    rows: Option<u32>,
    ai_tool_id: Option<String>,
    parent_session_id: Option<String>,
}

pub(super) async fn handle_create_admin_terminal(
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
