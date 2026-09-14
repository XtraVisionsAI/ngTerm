//! Login, admin login, token check and password change.

use super::*;

// --- Auth handlers ---

pub(super) async fn handle_login(
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

pub(super) async fn handle_admin_login(
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

pub(super) async fn handle_check(
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
pub(super) struct ChangePasswordRequest {
    old_password: String,
    new_password: String,
}

pub(super) async fn handle_change_password(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    let user_id = caller.user_id.clone();
    let cp_state = state.clone();
    let uid = user_id.clone();
    let change = Change::new(
        ObjectKind::User,
        Action::PasswordChange,
        "change own password",
    )
    .object_id(&user_id);
    let result = audit_config::run(&state, &caller, change, async move {
        auth::run_blocking(move || {
            auth::change_password(
                &cp_state.db,
                &uid,
                &req.old_password,
                &req.new_password,
                &cp_state.config.pepper,
            )
        })
        .await
        .map(Applied::new)
    })
    .await;
    match result {
        Ok(()) => {
            state.auth_sessions.write().await.remove(&user_id);
            Json(serde_json::json!({"message": "Password changed. Please login again."}))
                .into_response()
        }
        Err(e) => config_error_response(e, StatusCode::BAD_REQUEST),
    }
}
