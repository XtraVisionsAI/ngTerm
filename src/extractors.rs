use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

use crate::auth;
use crate::AppState;

#[derive(Serialize)]
struct ExtractorError {
    error: String,
}

fn err_response(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(ExtractorError {
            error: msg.to_string(),
        }),
    )
        .into_response()
}

fn extract_token_from_parts(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or_else(|| {
            parts
                .uri
                .query()
                .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")))
                .map(|s| s.to_string())
        })
}

/// Extracts authenticated user_id from Authorization header.
pub struct AuthUser(pub String);

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token_from_parts(parts)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Missing token"))?;
        let (user_id, _role) = auth::verify_token(&token, &state.config.jwt_secret)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Invalid token"))?;
        Ok(AuthUser(user_id))
    }
}

/// Verifies the request comes from an admin user.
pub struct AdminUser;

impl FromRequestParts<Arc<AppState>> for AdminUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token_from_parts(parts)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Missing token"))?;
        let (_user_id, role) = auth::verify_token(&token, &state.config.jwt_secret)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Invalid token"))?;
        if role != "admin" {
            return Err(err_response(StatusCode::FORBIDDEN, "Admin required"));
        }
        Ok(AdminUser)
    }
}

/// Extracts user_id + user_secret (for key decryption operations).
#[allow(dead_code)]
pub struct AuthUserWithSecret {
    pub user_id: String,
    pub user_secret: [u8; 32],
}

impl FromRequestParts<Arc<AppState>> for AuthUserWithSecret {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token_from_parts(parts)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Missing token"))?;
        let (user_id, _role) = auth::verify_token(&token, &state.config.jwt_secret)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Invalid token"))?;
        let user_secret = state
            .auth_sessions
            .read()
            .await
            .get_user_secret(&user_id)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Session expired"))?;
        Ok(AuthUserWithSecret {
            user_id,
            user_secret,
        })
    }
}

/// Authenticated caller with role, for endpoints whose data scope depends
/// on who is asking (audit listings, exports).
pub struct Caller {
    pub user_id: String,
    pub is_admin: bool,
}

impl Caller {
    /// Filter value pinning non-admins to their own rows.
    pub fn scope_user_id(&self) -> Option<String> {
        if self.is_admin {
            None
        } else {
            Some(self.user_id.clone())
        }
    }

    /// Whether a row owned by `owner` is visible to this caller. Rows with
    /// no owner are admin-only.
    pub fn can_see(&self, owner: Option<&str>) -> bool {
        self.is_admin || owner == Some(self.user_id.as_str())
    }
}

impl FromRequestParts<Arc<AppState>> for Caller {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token_from_parts(parts)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Missing token"))?;
        let (user_id, role) = auth::verify_token(&token, &state.config.jwt_secret)
            .ok_or_else(|| err_response(StatusCode::UNAUTHORIZED, "Invalid token"))?;
        Ok(Caller {
            user_id,
            is_admin: role == "admin",
        })
    }
}
