//! Legacy connection audit log listing and filters.

use super::*;

// --- Audit handlers ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AuditQuery {
    limit: Option<u32>,
    offset: Option<u32>,
    server: Option<String>,
    status: Option<String>,
    time_from: Option<String>,
    time_to: Option<String>,
    username: Option<String>,
}

pub(super) async fn handle_list_audit_logs(
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

pub(super) async fn handle_audit_filters(
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
