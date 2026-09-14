//! Interactive session lifecycle (create / list / close).

use super::*;

// --- Session handlers ---

pub(super) async fn handle_list_sessions(
    State(state): State<Arc<AppState>>,
    AuthUser(user_id): AuthUser,
) -> impl IntoResponse {
    let sessions = state.sessions.list_sessions(&user_id);
    Json(sessions).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateSessionRequest {
    server_id: String,
    cols: Option<u32>,
    rows: Option<u32>,
    parent_session_id: Option<String>,
}

pub(super) async fn handle_create_session(
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

    // Pre-execution check: the installed guard may require a second person
    // to admit this user to this server before anything is connected.
    match crate::guard::check(
        state.guard(),
        crate::guard::GuardRequest {
            user_id: &user_id,
            is_admin: false,
            action: crate::guard::GuardAction::SessionAdmission {
                server_id: &server.id,
                remote_user: &server.username,
            },
        },
    )
    .await
    {
        crate::guard::GuardDecision::Proceed => {}
        other => return guard_response(other),
    }

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

pub(super) async fn handle_delete_session(
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
