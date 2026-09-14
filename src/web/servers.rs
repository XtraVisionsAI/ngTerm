//! Server registry: groups, CRUD, SSH config import, connection test.

use super::*;

// --- Server handlers ---

pub(super) async fn handle_list_groups(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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
pub(super) struct ServerQuery {
    group_name: Option<String>,
}

pub(super) async fn handle_list_servers(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
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

pub(super) async fn handle_create_server(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<CreateServerRequest>,
) -> impl IntoResponse {
    let change = Change::new(
        ObjectKind::Server,
        Action::Create,
        format!(
            "create server {} ({}@{})",
            req.alias, req.username, req.host
        ),
    )
    .target(audit_events::Target {
        server_alias: Some(req.alias.clone()),
        server_host: Some(req.host.clone()),
        remote_user: Some(req.username.clone()),
        ..Default::default()
    });
    let result = audit_config::run(&state, &caller, change, async {
        let server = server_registry::create_server(&state.db, &req)?;
        Ok(Applied::new(server.clone())
            .object_id(server.id.clone())
            .after(Some(audit_config::server_snapshot(&server))))
    })
    .await;
    match result {
        Ok(server) => (StatusCode::CREATED, Json(server)).into_response(),
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSshConfigRequest {
    pub text: String,
    /// Parse and match only; create nothing.
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub group_name: Option<String>,
    /// Key to assign to hosts whose IdentityFile did not match a stored key.
    #[serde(default)]
    pub default_key_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportCandidate {
    #[serde(flatten)]
    parsed: crate::ssh_config_import::ParsedHost,
    /// Stored key matched by IdentityFile name, or the default.
    key_id: Option<String>,
    key_matched_by_name: bool,
    /// An existing server already has this alias, or this user@host:port.
    exists: bool,
}

/// Import a declared subset of an OpenSSH client config. Always returns the
/// preview (candidates + skipped blocks with reasons); with `dryRun=false`
/// creates the candidates that do not already exist, tagged
/// `import:ssh-config`.
pub(super) async fn handle_import_ssh_config(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<ImportSshConfigRequest>,
) -> impl IntoResponse {
    if req.text.len() > 256 * 1024 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ApiError {
                error: "config text too large (max 256 KiB)".into(),
            }),
        )
            .into_response();
    }
    let parsed = crate::ssh_config_import::parse(&req.text);
    let keys = key_manager::list_keys(&state.db, &caller.user_id).unwrap_or_default();
    let existing = server_registry::list_servers(&state.db, None).unwrap_or_default();
    let group = req
        .group_name
        .clone()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty());

    let candidates: Vec<ImportCandidate> = parsed
        .hosts
        .into_iter()
        .map(|h| {
            let by_name = h.identity_file.as_deref().and_then(|f| {
                keys.iter()
                    .find(|k| k.name == f || k.name.trim_end_matches(".pub") == f)
                    .map(|k| k.id.clone())
            });
            let exists = existing.iter().any(|s| {
                s.alias == h.alias
                    || (s.host == h.host
                        && s.port == h.port
                        && h.username.as_deref().is_some_and(|u| u == s.username))
            });
            ImportCandidate {
                key_id: by_name.clone().or_else(|| req.default_key_id.clone()),
                key_matched_by_name: by_name.is_some(),
                exists,
                parsed: h,
            }
        })
        .collect();

    let mut created: Vec<server_registry::Server> = Vec::new();
    let mut errors: Vec<serde_json::Value> = Vec::new();
    if !req.dry_run {
        for c in candidates.iter().filter(|c| !c.exists) {
            let Some(username) = c.parsed.username.clone() else {
                errors.push(serde_json::json!({
                    "alias": c.parsed.alias, "error": "no User in the block; set one and retry"
                }));
                continue;
            };
            let create = server_registry::CreateServerRequest {
                group_name: group.clone(),
                alias: c.parsed.alias.clone(),
                host: c.parsed.host.clone(),
                port: Some(c.parsed.port),
                username: username.clone(),
                key_id: c.key_id.clone(),
                tags: Some(vec!["import:ssh-config".to_string()]),
                ai_tool_id: None,
            };
            let change = Change::new(
                ObjectKind::Server,
                Action::Create,
                format!(
                    "import server {} ({}@{}) from ssh config",
                    create.alias, username, create.host
                ),
            )
            .target(audit_events::Target {
                server_alias: Some(create.alias.clone()),
                server_host: Some(create.host.clone()),
                remote_user: Some(username.clone()),
                ..Default::default()
            });
            let result = audit_config::run(&state, &caller, change, async {
                let server = server_registry::create_server(&state.db, &create)?;
                Ok(Applied::new(server.clone())
                    .object_id(server.id.clone())
                    .after(Some(audit_config::server_snapshot(&server))))
            })
            .await;
            match result {
                Ok(s) => created.push(s),
                Err(e) => errors.push(serde_json::json!({
                    "alias": c.parsed.alias, "error": format!("{:?}", e)
                })),
            }
        }
    }
    Json(serde_json::json!({
        "candidates": candidates,
        "skipped": parsed.skipped,
        "created": created,
        "errors": errors,
    }))
    .into_response()
}

pub(super) async fn handle_delete_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    caller: Caller,
) -> impl IntoResponse {
    let existing = server_registry::get_server(&state.db, &id).ok().flatten();
    let alias = existing
        .as_ref()
        .map(|s| s.alias.clone())
        .unwrap_or_else(|| id.clone());
    let change = Change::new(
        ObjectKind::Server,
        Action::Delete,
        format!("delete server {}", alias),
    )
    .object_id(&id)
    .target(
        existing
            .as_ref()
            .map(audit_config::server_target)
            .unwrap_or_default(),
    )
    .before(existing.as_ref().map(audit_config::server_snapshot));
    let result = audit_config::run(&state, &caller, change, async {
        match server_registry::delete_server(&state.db, &id) {
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

pub(super) async fn handle_update_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    caller: Caller,
    Json(req): Json<server_registry::UpdateServerRequest>,
) -> impl IntoResponse {
    let existing = server_registry::get_server(&state.db, &id).ok().flatten();
    let alias = existing
        .as_ref()
        .map(|s| s.alias.clone())
        .unwrap_or_else(|| id.clone());
    let change = Change::new(
        ObjectKind::Server,
        Action::Update,
        format!("update server {}", alias),
    )
    .object_id(&id)
    .target(
        existing
            .as_ref()
            .map(audit_config::server_target)
            .unwrap_or_default(),
    )
    .before(existing.as_ref().map(audit_config::server_snapshot));
    let result = audit_config::run(&state, &caller, change, async {
        match server_registry::update_server(&state.db, &id, &req) {
            Ok(true) => {
                let after = server_registry::get_server(&state.db, &id)
                    .ok()
                    .flatten()
                    .map(|s| audit_config::server_snapshot(&s));
                Ok(Applied::new(()).after(after))
            }
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

pub(super) async fn handle_test_server(
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
