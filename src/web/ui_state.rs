//! Per-user persisted UI state.

use super::*;

// --- UI State ---

pub(super) async fn handle_get_ui_state(
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

pub(super) async fn handle_save_ui_state(
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
