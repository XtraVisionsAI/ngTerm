//! SSH key management.

use super::*;

// --- Key handlers ---

pub(super) async fn handle_list_keys(
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

pub(super) async fn handle_create_key(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let user_secret = match state.auth_sessions.read().await.get_user_secret(&user_id) {
        Some(key) => key,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let change = Change::new(
        ObjectKind::SshKey,
        Action::Create,
        format!("add ssh key {}", req.name),
    );
    let result = audit_config::run(&state, &caller, change, async {
        let info = key_manager::create_key(
            &state.db,
            &user_id,
            &user_secret,
            &req.name,
            &req.private_key,
        )?;
        tracing::info!("Key created: id={}, type={}", info.id, info.key_type);
        let after = audit_config::key_snapshot(&info);
        Ok(Applied::new(info)
            .object_id(after["id"].as_str().unwrap_or_default().to_string())
            .after(Some(after)))
    })
    .await;
    match result {
        Ok(info) => (StatusCode::CREATED, Json(info)).into_response(),
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}

pub(super) async fn handle_delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    caller: Caller,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let before = key_manager::list_keys(&state.db, &user_id)
        .ok()
        .and_then(|keys| keys.into_iter().find(|k| k.id == id))
        .map(|k| audit_config::key_snapshot(&k));
    let name = before
        .as_ref()
        .and_then(|b| b.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    let change = Change::new(
        ObjectKind::SshKey,
        Action::Delete,
        format!("delete ssh key {}", name),
    )
    .object_id(&id)
    .before(before);
    let result = audit_config::run(&state, &caller, change, async {
        match key_manager::delete_key(&state.db, &user_id, &id) {
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
