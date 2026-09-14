//! File browser on the session helper channel: list, read, verified write, restore, upload/download.

use super::*;

// --- File browser handlers ---

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WriteFileRequest {
    path: String,
    content: String,
    /// Hash the client previewed against (`file_ops::Baseline::binding`):
    /// the write is refused with 409 when the file differs now.
    baseline_sha256: Option<String>,
    /// Keep a copy of the current content next to the file (default true
    /// when the file exists).
    backup: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MkdirRequest {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RenameRequest {
    from: String,
    to: String,
}

/// Make sure the helper connection (SFTP/exec channels next to the user's
/// terminal) for `session_id` exists, opening it with the caller's key when
/// needed. Public so distributions can run their own session-scoped
/// operations through the same connection.
pub async fn ensure_helper(
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

pub(super) async fn handle_list_files(
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

pub(super) async fn handle_read_file(
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

    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::FileRead,
        format!("read {}", query.path),
        None,
        state.helpers.sftp_read(&session_id, &query.path),
    )
    .await
    {
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
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_write_file(
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

    // What the file is right now. A client that previewed a diff sends the
    // hash it previewed against; a different file now means the preview no
    // longer describes the change and the write is refused.
    let baseline = match file_ops::read_baseline(&state, &session_id, &req.path).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };
    if let Some(expected) = req.baseline_sha256.as_deref() {
        if expected != baseline.binding() {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "file changed since it was previewed; reload and review again",
                    "code": "baseline_mismatch",
                    "baseline": baseline,
                })),
            )
                .into_response();
        }
    }
    let want_backup = req.backup.unwrap_or(true) && baseline.exists;
    let summary = format!(
        "write {} ({} bytes; baseline {})",
        req.path,
        req.content.len(),
        baseline.short_binding()
    );
    verified_write(
        &state,
        &session_id,
        &user_id,
        &req.path,
        req.content.as_bytes(),
        summary,
        baseline,
        want_backup,
    )
    .await
}

/// Managed file write with backup and read-back verification. The
/// operation is recorded before anything is sent; every step leaves an
/// event on it, and a write whose read-back does not match is a *failed*
/// operation with the evidence attached, not a success.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verified_write(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
    path: &str,
    content: &[u8],
    summary: String,
    baseline: file_ops::Baseline,
    want_backup: bool,
) -> axum::response::Response {
    let managed = match audit_ops::run_begin(
        state,
        session_id,
        user_id,
        OperationKind::FileWrite,
        summary,
        None,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return op_error_response(e),
    };
    let op_id = managed.id().to_string();
    file_ops::record_event(
        &state.db,
        session_id,
        &op_id,
        file_ops::EVENT_BASELINE,
        serde_json::json!({
            "path": path,
            "exists": baseline.exists,
            "sha256": baseline.sha256,
            "size": baseline.size,
            "tooLarge": baseline.too_large,
        }),
    );
    let mut backup_path = None;
    if want_backup {
        match file_ops::backup_file(state, session_id, path).await {
            Ok((bp, sha)) => {
                file_ops::record_event(
                    &state.db,
                    session_id,
                    &op_id,
                    file_ops::EVENT_BACKUP,
                    serde_json::json!({ "backupPath": bp, "sha256": sha }),
                );
                backup_path = Some(bp);
            }
            Err(e) => {
                // Nothing was written: the file is untouched.
                managed.failed(&format!("{}; nothing written", e));
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError {
                        error: format!("{}; the file was not modified", e),
                    }),
                )
                    .into_response();
            }
        }
    }
    if let Err(e) = state.helpers.sftp_write(session_id, path, content).await {
        managed.failed(&e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: e }),
        )
            .into_response();
    }
    let expected = file_ops::sha256_hex(content);
    match file_ops::read_back_hash(state, session_id, path).await {
        Ok(actual) if actual == expected => {
            file_ops::record_event(
                &state.db,
                session_id,
                &op_id,
                file_ops::EVENT_VERIFIED,
                serde_json::json!({ "sha256": actual, "bytes": content.len() }),
            );
            managed.succeeded();
            Json(file_ops::WriteReport {
                operation_id: op_id,
                path: path.to_string(),
                sha256: expected,
                verified: true,
                backup_path,
                baseline,
            })
            .into_response()
        }
        Ok(actual) => {
            file_ops::record_event(
                &state.db,
                session_id,
                &op_id,
                file_ops::EVENT_VERIFY_FAILED,
                serde_json::json!({ "expected": expected, "actual": actual, "backupPath": backup_path }),
            );
            managed.failed(
                "post-write verification failed: content on disk differs from what was written",
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "the file was written but reads back differently; check the target and the backup",
                    "code": "verify_failed",
                    "operationId": op_id,
                    "expectedSha256": expected,
                    "actualSha256": actual,
                    "backupPath": backup_path,
                })),
            )
                .into_response()
        }
        Err(e) => {
            file_ops::record_event(
                &state.db,
                session_id,
                &op_id,
                file_ops::EVENT_VERIFY_FAILED,
                serde_json::json!({ "expected": expected, "error": e, "backupPath": backup_path }),
            );
            managed.failed(&format!("write sent but could not be verified: {}", e));
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("the write was sent but could not be verified: {}", e),
                    "code": "verify_failed",
                    "operationId": op_id,
                    "backupPath": backup_path,
                })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestoreFileRequest {
    path: String,
    backup_path: String,
}

/// Put a backup made by a previous write back in place. Itself a verified
/// write (with its own backup of the current content), so a restore can be
/// undone too.
pub(super) async fn handle_restore_file(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    AuthUser(user_id): AuthUser,
    Json(req): Json<RestoreFileRequest>,
) -> impl IntoResponse {
    if !state.sessions.is_owner(&session_id, &user_id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Err(e) = ensure_helper(&state, &session_id, &user_id).await {
        return e.into_response();
    }
    for p in [&req.path, &req.backup_path] {
        if let Err(e) = crate::utils::validate_path(p) {
            return (StatusCode::BAD_REQUEST, Json(ApiError { error: e })).into_response();
        }
    }
    if !req.backup_path.contains(".ngterm-bak-") {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "backupPath is not a backup made by this platform".into(),
            }),
        )
            .into_response();
    }
    let data = match state.helpers.sftp_read(&session_id, &req.backup_path).await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: format!("backup not readable: {}", e),
                }),
            )
                .into_response()
        }
    };
    let baseline = match file_ops::read_baseline(&state, &session_id, &req.path).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response()
        }
    };
    let summary = format!(
        "restore {} from {} ({} bytes; baseline {})",
        req.path,
        req.backup_path,
        data.len(),
        baseline.short_binding()
    );
    let exists = baseline.exists;
    verified_write(
        &state,
        &session_id,
        &user_id,
        &req.path,
        &data,
        summary,
        baseline,
        exists,
    )
    .await
}

pub(super) async fn handle_delete_file(
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

    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::FileDelete,
        format!("delete {}", query.path),
        None,
        state.helpers.sftp_delete(&session_id, &query.path),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_mkdir(
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

    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Mkdir,
        format!("mkdir {}", req.path),
        None,
        state.helpers.sftp_mkdir(&session_id, &req.path),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_rename_file(
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

    match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::FileRename,
        format!("rename {} -> {}", req.from, req.to),
        None,
        state.helpers.sftp_rename(&session_id, &req.from, &req.to),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => op_error_response(e),
    }
}

pub(super) async fn handle_home_dir(
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

pub(super) async fn handle_stat_file(
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

pub(super) async fn handle_download_file(
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

    // Recorded when the remote file is opened: the transfer itself streams
    // after the response starts and its completion is not observed here.
    let file = match audit_ops::run(
        &state,
        &session_id,
        &user_id,
        OperationKind::Download,
        format!("download {} ({} bytes)", query.path, file_size),
        None,
        state.helpers.open_read(&session_id, &query.path),
    )
    .await
    {
        Ok(f) => f,
        Err(e) => return op_error_response(e),
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

pub(super) async fn handle_upload_file(
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

        let managed = match audit_ops::run_begin(
            &state,
            &session_id,
            &user_id,
            OperationKind::Upload,
            format!("upload {}", remote_path),
            None,
        )
        .await
        {
            Ok(m) => m,
            Err(e) => return op_error_response(e),
        };
        let mut file = match state.helpers.open_write(&session_id, &remote_path).await {
            Ok(f) => f,
            Err(e) => {
                managed.failed(&e);
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
            managed.failed(&e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: e }),
            )
                .into_response();
        }
        managed.succeeded();

        uploaded.push(serde_json::json!({
            "name": file_name,
            "path": remote_path,
            "size": total_written,
        }));
    }

    Json(serde_json::json!({ "uploaded": uploaded })).into_response()
}
