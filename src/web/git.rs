//! Read-only Git views on the session helper channel.

use super::*;

// --- Git handlers ---

#[derive(Deserialize)]
pub(super) struct GitLogQuery {
    limit: Option<u32>,
}

pub(super) async fn handle_git_status(
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
    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Git,
        cmd.to_string(),
        None,
        state.helpers.exec(&session_id, cmd),
    )
    .await
    {
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
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_git_log(
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
    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Git,
        cmd.clone(),
        None,
        state.helpers.exec(&session_id, &cmd),
    )
    .await
    {
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
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_git_branches(
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
    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Git,
        cmd.to_string(),
        None,
        state.helpers.exec(&session_id, cmd),
    )
    .await
    {
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
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_git_diff(
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
    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Git,
        cmd.to_string(),
        None,
        state.helpers.exec(&session_id, cmd),
    )
    .await
    {
        Ok(diff) => Json(serde_json::json!({ "diff": diff })).into_response(),
        Err(e) => op_error_response(e),
    }
}
