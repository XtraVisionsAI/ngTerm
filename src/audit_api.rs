//! Audit query, export and recording-access API (AUD-05) with per-user
//! scoping (AUD-07 basics).
//!
//! Every listing goes through the same filter as its export so the two can
//! never disagree. Non-admin callers are pinned to their own user id at the
//! filter level, not by post-filtering rows. Reading a recording or exporting
//! is itself recorded as an audit event on the `audit-access` stream.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::audit_events::{
    self, ActorKind, AuditSession, Integrity, NewEvent, OperationFilter, OperationKind,
    OperationRecord, OperationStatus, SessionFilter, Source,
};
use crate::extractors::Caller;
use crate::recording::{ChunkMeta, ChunkProblem, RecordingMeta};
use crate::AppState;

const MAX_PAGE: u32 = 200;
/// Hard bound on export rows; the response says whether it was hit.
const MAX_EXPORT_ROWS: u32 = 10_000;
const ACCESS_STREAM: &str = "audit-access";

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// Boxed early-return response, so `Result<_, Reject>` stays small.
type Reject = Box<Response>;

fn reject(status: StatusCode, msg: impl Into<String>) -> Reject {
    Box::new(err(status, msg))
}

fn parse_enum<T: for<'de> Deserialize<'de>>(s: &Option<String>) -> Result<Option<T>, Reject> {
    match s {
        None => Ok(None),
        Some(v) => serde_json::from_value(serde_json::Value::String(v.clone()))
            .map(Some)
            .map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    format!("invalid filter value {:?}", v),
                )
            }),
    }
}

fn record_access(state: &AppState, caller: &Caller, event_type: &str, payload: serde_json::Value) {
    let mut payload = payload;
    payload["userId"] = caller.user_id.clone().into();
    payload["admin"] = caller.is_admin.into();
    if let Err(e) = audit_events::append_event(
        &state.db,
        NewEvent {
            stream_id: ACCESS_STREAM,
            occurred_at: None,
            session_id: None,
            operation_id: None,
            event_type,
            payload,
            integrity: Integrity::Complete,
        },
    ) {
        tracing::error!("Failed to record audit access event {}: {}", event_type, e);
    }
}

// --- Sessions ---------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuery {
    pub username: Option<String>,
    pub server: Option<String>,
    pub remote_user: Option<String>,
    pub source: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    pub active: Option<bool>,
    pub integrity: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl SessionQuery {
    fn to_filter(&self, caller: &Caller) -> Result<SessionFilter, Reject> {
        Ok(SessionFilter {
            user_id: caller.scope_user_id(),
            username: if caller.is_admin {
                self.username.clone()
            } else {
                None
            },
            server_id: self.server.clone(),
            remote_user: self.remote_user.clone(),
            source: parse_enum::<Source>(&self.source)?,
            time_from: self.time_from.clone(),
            time_to: self.time_to.clone(),
            active: self.active,
            integrity: self.integrity.clone(),
        })
    }
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<SessionQuery>,
) -> Response {
    let filter = match q.to_filter(&caller) {
        Ok(f) => f,
        Err(r) => return *r,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0);
    match audit_events::list_sessions(&state.db, &filter, limit, offset) {
        Ok((items, total)) => {
            Json(serde_json::json!({ "items": items, "total": total })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Load a session the caller may see, or the response explaining why not.
/// Unknown and foreign sessions are indistinguishable to non-admins.
fn visible_session(
    state: &AppState,
    caller: &Caller,
    session_id: &str,
) -> Result<AuditSession, Reject> {
    match audit_events::get_session(&state.db, session_id) {
        Ok(Some(s)) if caller.can_see(s.actor.user_id.as_deref()) => Ok(s),
        Ok(_) => Err(reject(StatusCode::NOT_FOUND, "Session not found")),
        Err(e) => Err(reject(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(session_id): Path<String>,
) -> Response {
    let session = match visible_session(&state, &caller, &session_id) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let ops_filter = OperationFilter {
        session_id: Some(session_id.clone()),
        ..Default::default()
    };
    let operations = match audit_events::list_operations(&state.db, &ops_filter, MAX_PAGE, 0) {
        Ok((ops, _)) => ops,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let recordings = match recordings_for(&state, &session_id) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    Json(serde_json::json!({
        "session": session,
        "operations": operations,
        "recordings": recordings,
    }))
    .into_response()
}

fn recordings_for(state: &AppState, session_id: &str) -> Result<Vec<RecordingMeta>, String> {
    match &state.recordings {
        Some(store) => store.list_for_session(session_id),
        None => Ok(Vec::new()),
    }
}

// --- Operations ---------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OperationQuery {
    /// Substring of the (redacted) command / summary.
    pub q: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub actor_kind: Option<String>,
    pub session: Option<String>,
    pub server: Option<String>,
    pub task: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl OperationQuery {
    fn to_filter(&self, caller: &Caller) -> Result<OperationFilter, Reject> {
        Ok(OperationFilter {
            user_id: caller.scope_user_id(),
            session_id: self.session.clone(),
            task_id: self.task.clone(),
            actor_kind: parse_enum::<ActorKind>(&self.actor_kind)?,
            status: parse_enum::<OperationStatus>(&self.status)?,
            kind: parse_enum::<OperationKind>(&self.kind)?,
            server_id: self.server.clone(),
            time_from: self.time_from.clone(),
            time_to: self.time_to.clone(),
            summary_contains: self.q.clone().filter(|s| !s.trim().is_empty()),
        })
    }
}

pub async fn list_operations(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<OperationQuery>,
) -> Response {
    let filter = match q.to_filter(&caller) {
        Ok(f) => f,
        Err(r) => return *r,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0);
    match audit_events::list_operations(&state.db, &filter, limit, offset) {
        Ok((items, total)) => {
            Json(serde_json::json!({ "items": items, "total": total })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Where an operation sits inside its session's recording, so a command hit
/// can be opened at the right moment in the player.
fn locate_in_recording(
    state: &AppState,
    op: &OperationRecord,
) -> Result<Option<serde_json::Value>, String> {
    let (Some(store), Some(session_id)) = (&state.recordings, &op.session_id) else {
        return Ok(None);
    };
    let started = chrono::DateTime::parse_from_rfc3339(&op.started_at).ok();
    for rec in store.list_for_session(session_id)? {
        let Some(rec_start) = chrono::DateTime::parse_from_rfc3339(&rec.started_at).ok() else {
            continue;
        };
        let Some(op_start) = started else { break };
        let offset_ms = (op_start - rec_start).num_milliseconds().max(0) as u64;
        let within = match rec.duration_ms {
            Some(d) => offset_ms <= d,
            None => true, // still recording
        };
        if within {
            return Ok(Some(serde_json::json!({
                "recordingId": rec.recording_id,
                "offsetMs": offset_ms,
                "integrity": rec.integrity,
            })));
        }
    }
    Ok(None)
}

pub async fn get_operation(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(operation_id): Path<String>,
) -> Response {
    let op = match audit_events::get_operation(&state.db, &operation_id) {
        Ok(Some(op)) if caller.can_see(op.actor.user_id.as_deref()) => op,
        Ok(_) => return err(StatusCode::NOT_FOUND, "Operation not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let events = match audit_events::events_for_operation(&state.db, &operation_id) {
        Ok(ev) => ev,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let recording = match locate_in_recording(&state, &op) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    Json(serde_json::json!({
        "operation": op,
        "events": events,
        "recording": recording,
    }))
    .into_response()
}

// --- System -------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemQuery {
    pub limit: Option<u32>,
}

/// Start/shutdown events with what each start had to recover. Admin only:
/// they describe the process, not any user's activity.
pub async fn list_system_events(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<SystemQuery>,
) -> Response {
    if !caller.is_admin {
        return err(StatusCode::FORBIDDEN, "Admin required");
    }
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_PAGE);
    match audit_events::recent_events(&state.db, crate::audit_system::SYSTEM_STREAM, limit) {
        Ok(items) => Json(serde_json::json!({
            "items": items,
            "current": state.startup,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityQuery {
    /// How many of the newest recordings to verify chunk by chunk (max 200).
    pub recordings: Option<u32>,
}

/// On-demand integrity check: append-only guards, stream continuity and
/// chunk hashes of recent recordings. Admin only; the check is itself
/// recorded as an access event.
pub async fn integrity(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<IntegrityQuery>,
) -> Response {
    if !caller.is_admin {
        return err(StatusCode::FORBIDDEN, "Admin required");
    }
    let limit = q.recordings.unwrap_or(50).clamp(0, MAX_PAGE);
    let db = state.db.clone();
    let store = state.recordings.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::audit_maintenance::integrity_report(&db, store.as_ref(), limit)
    })
    .await;
    match report {
        Ok(Ok(report)) => {
            record_access(
                &state,
                &caller,
                "audit.integrity_check",
                serde_json::json!({
                    "recordingsChecked": report.recordings_checked,
                    "recordingsWithProblems": report.recordings_with_problems.len(),
                    "streamGaps": report.stream_gaps.len(),
                    "guardsPresent": report.append_only_guards_present,
                }),
            );
            Json(report).into_response()
        }
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// --- Recordings ---------------------------------------------------------------

/// A recording the caller may read, resolved through its session's owner.
fn visible_recording(
    state: &AppState,
    caller: &Caller,
    recording_id: &str,
) -> Result<(Arc<crate::recording::RecordingStore>, RecordingMeta), Reject> {
    let store = state
        .recordings
        .clone()
        .ok_or_else(|| reject(StatusCode::NOT_FOUND, "Recording not found"))?;
    let meta = match store.get(recording_id) {
        Ok(Some(m)) => m,
        Ok(None) => return Err(reject(StatusCode::NOT_FOUND, "Recording not found")),
        Err(e) => return Err(reject(StatusCode::INTERNAL_SERVER_ERROR, e)),
    };
    visible_session(state, caller, &meta.session_id)?;
    Ok((store, meta))
}

pub async fn list_recordings(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(session_id): Path<String>,
) -> Response {
    if let Err(r) = visible_session(&state, &caller, &session_id) {
        return *r;
    }
    let Some(store) = &state.recordings else {
        return Json(serde_json::json!({ "items": [] })).into_response();
    };
    let metas = match store.list_for_session(&session_id) {
        Ok(m) => m,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let mut items = Vec::new();
    for meta in metas {
        let chunks: Vec<ChunkMeta> = store.chunks(&meta.recording_id).unwrap_or_default();
        let problems: Vec<ChunkProblem> = store.verify(&meta.recording_id).unwrap_or_default();
        items.push(serde_json::json!({
            "recording": meta,
            "chunks": chunks,
            "problems": problems,
        }));
    }
    Json(serde_json::json!({ "items": items })).into_response()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventsQuery {
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
}

/// Events of a recording for playback. Chunks that fail verification are
/// skipped and reported in `problems`; the caller must show the gap, not
/// paper over it.
pub async fn recording_events(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(recording_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Response {
    let (store, meta) = match visible_recording(&state, &caller, &recording_id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let chunks = match store.chunks(&recording_id) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let from = q.from_ms.unwrap_or(0);
    let to = q.to_ms.unwrap_or(u64::MAX);
    let mut events = Vec::new();
    let mut problems = Vec::new();
    let mut expected = 1u32;
    for chunk in &chunks {
        while expected < chunk.seq {
            problems.push(ChunkProblem::IndexGap {
                expected_seq: expected,
            });
            expected += 1;
        }
        expected = chunk.seq + 1;
        if chunk.end_ms < from || chunk.start_ms > to {
            continue;
        }
        match store.read_chunk(chunk) {
            Ok(evs) => events.extend(
                evs.into_iter()
                    .filter(|e| e.t_ms >= from && e.t_ms <= to)
                    .map(|e| e.to_json()),
            ),
            Err(detail) => {
                let path = store.config().root.join(&chunk.path);
                problems.push(if path.exists() {
                    ChunkProblem::Corrupt {
                        seq: chunk.seq,
                        detail,
                    }
                } else {
                    ChunkProblem::Missing { seq: chunk.seq }
                });
            }
        }
    }
    record_access(
        &state,
        &caller,
        "audit.recording_read",
        serde_json::json!({
            "recordingId": recording_id,
            "sessionId": meta.session_id,
            "fromMs": from,
            "toMs": q.to_ms,
            "events": events.len(),
        }),
    );
    Json(serde_json::json!({
        "recording": meta,
        "events": events,
        "problems": problems,
    }))
    .into_response()
}

// --- Export -------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportQuery {
    /// `sessions` or `operations`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `json` (default) or `csv`.
    pub format: Option<String>,
    #[serde(flatten)]
    pub sessions: SessionQuery,
    #[serde(flatten)]
    pub operations: OperationQuery,
}

fn csv_field(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Neutralise spreadsheet formula injection as well as quoting.
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{}", s)
    } else {
        s
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// CSV header name and the JSON path it is read from.
type Columns = &'static [(&'static str, &'static [&'static str])];

fn to_csv(columns: Columns, rows: &[serde_json::Value]) -> String {
    let mut out = String::new();
    out.push_str(
        &columns
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for row in rows {
        let line: Vec<String> = columns
            .iter()
            .map(|(_, path)| {
                let mut v = row;
                for key in path.iter() {
                    v = &v[*key];
                }
                csv_field(v)
            })
            .collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

const SESSION_COLUMNS: Columns = &[
    ("sessionId", &["sessionId"]),
    ("connectedAt", &["connectedAt"]),
    ("disconnectedAt", &["disconnectedAt"]),
    ("disconnectReason", &["disconnectReason"]),
    ("userId", &["actor", "user_id"]),
    ("username", &["actor", "username"]),
    ("remoteAddr", &["actor", "remote_addr"]),
    ("source", &["source"]),
    ("serverId", &["target", "server_id"]),
    ("serverAlias", &["target", "server_alias"]),
    ("serverHost", &["target", "server_host"]),
    ("remoteUser", &["target", "remote_user"]),
    ("integrity", &["integrity", "kind"]),
];

const OPERATION_COLUMNS: Columns = &[
    ("operationId", &["operationId"]),
    ("startedAt", &["startedAt"]),
    ("finishedAt", &["finishedAt"]),
    ("status", &["status"]),
    ("kind", &["kind"]),
    ("summary", &["summary"]),
    ("actorKind", &["actor", "kind"]),
    ("userId", &["actor", "user_id"]),
    ("username", &["actor", "username"]),
    ("toolId", &["actor", "tool_id"]),
    ("source", &["source"]),
    ("serverId", &["target", "server_id"]),
    ("serverAlias", &["target", "server_alias"]),
    ("remoteUser", &["target", "remote_user"]),
    ("cwd", &["cwd"]),
    ("exit", &["exit"]),
    ("evidence", &["evidence"]),
    ("sessionId", &["sessionId"]),
    ("taskId", &["taskId"]),
];

pub async fn export(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Query(q): Query<ExportQuery>,
) -> Response {
    let format = q.format.as_deref().unwrap_or("json");
    if !matches!(format, "json" | "csv") {
        return err(StatusCode::BAD_REQUEST, "format must be json or csv");
    }
    let (rows, total, columns): (Vec<serde_json::Value>, u32, Columns) = match q.kind.as_str() {
        "sessions" => {
            let filter = match q.sessions.to_filter(&caller) {
                Ok(f) => f,
                Err(r) => return *r,
            };
            match audit_events::list_sessions(&state.db, &filter, MAX_EXPORT_ROWS, 0) {
                Ok((items, total)) => (
                    items
                        .iter()
                        .filter_map(|s| serde_json::to_value(s).ok())
                        .collect(),
                    total,
                    SESSION_COLUMNS,
                ),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        "operations" => {
            let filter = match q.operations.to_filter(&caller) {
                Ok(f) => f,
                Err(r) => return *r,
            };
            match audit_events::list_operations(&state.db, &filter, MAX_EXPORT_ROWS, 0) {
                Ok((items, total)) => (
                    items
                        .iter()
                        .filter_map(|s| serde_json::to_value(s).ok())
                        .collect(),
                    total,
                    OPERATION_COLUMNS,
                ),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        other => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("unknown export type {:?}", other),
            )
        }
    };
    let truncated = total > rows.len() as u32;
    record_access(
        &state,
        &caller,
        "audit.export",
        serde_json::json!({
            "type": q.kind,
            "format": format,
            "rows": rows.len(),
            "total": total,
            "truncated": truncated,
        }),
    );

    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("ngterm-audit-{}-{}.{}", q.kind, stamp, format);
    let disposition = format!("attachment; filename=\"{}\"", filename);
    match format {
        "csv" => {
            let mut body = to_csv(columns, &rows);
            if truncated {
                body.push_str(&format!(
                    "# truncated: {} of {} rows exported\n",
                    rows.len(),
                    total
                ));
            }
            (
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                    (
                        header::HeaderName::from_static("x-audit-truncated"),
                        truncated.to_string(),
                    ),
                ],
                body,
            )
                .into_response()
        }
        _ => (
            [(header::CONTENT_DISPOSITION, disposition)],
            Json(serde_json::json!({
                "type": q.kind,
                "items": rows,
                "total": total,
                "truncated": truncated,
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_events::{Actor, OperationIntent, Target};

    async fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("ngterm-audapi-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Database::open(dir.to_str().unwrap()).unwrap();
        let config = crate::config::AppConfig {
            default_cols: 80,
            default_rows: 24,
            data_dir: dir.to_string_lossy().to_string(),
            pepper: "p".into(),
            jwt_secret: vec![1; 32],
            recording: crate::recording::RecordingConfig::for_data_dir(&dir.to_string_lossy()),
        };
        let (state, _rx) = crate::build_app_state(config, db).await;
        state
    }

    fn user(id: &str) -> Caller {
        Caller {
            user_id: id.into(),
            is_admin: false,
        }
    }

    fn admin() -> Caller {
        Caller {
            user_id: "admin".into(),
            is_admin: true,
        }
    }

    async fn body_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    fn seed_session(state: &AppState, session_id: &str, user_id: &str) -> String {
        audit_events::session_started(
            &state.db,
            &AuditSession {
                session_id: session_id.into(),
                actor: Actor {
                    kind: Some(ActorKind::Human),
                    user_id: Some(user_id.into()),
                    username: Some(user_id.into()),
                    ..Default::default()
                },
                target: Target {
                    server_id: Some("srv".into()),
                    ..Default::default()
                },
                source: Source::Terminal,
                parent_session_id: None,
                connected_at: chrono::Utc::now().to_rfc3339(),
                disconnected_at: None,
                disconnect_reason: None,
                integrity: Integrity::Complete,
            },
        )
        .unwrap();
        audit_events::operation_intended(
            &state.db,
            &OperationIntent {
                session_id: Some(session_id.into()),
                task_id: None,
                parent_operation_id: None,
                actor: Actor {
                    kind: Some(ActorKind::Human),
                    user_id: Some(user_id.into()),
                    ..Default::default()
                },
                source: Source::Terminal,
                kind: OperationKind::Command,
                summary: format!("rm -rf /tmp/{}", user_id),
                target: Target::default(),
                cwd: None,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn non_admins_only_see_their_own_rows_and_export_matches_list() {
        let state = test_state().await;
        let op1 = seed_session(&state, "s1", "u1");
        let op2 = seed_session(&state, "s2", "u2");

        let (st, v) = body_json(
            list_sessions(
                State(state.clone()),
                user("u1"),
                Query(SessionQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["total"], 1);
        assert_eq!(v["items"][0]["sessionId"], "s1");

        // Foreign rows are indistinguishable from missing ones.
        let (st, _) =
            body_json(get_session(State(state.clone()), user("u1"), Path("s2".into())).await).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) =
            body_json(get_operation(State(state.clone()), user("u1"), Path(op2.clone())).await)
                .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, v) =
            body_json(get_operation(State(state.clone()), user("u1"), Path(op1.clone())).await)
                .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["operation"]["summary"], "rm -rf /tmp/u1");

        // A username filter cannot widen a non-admin's scope.
        let q = OperationQuery {
            q: Some("rm -rf".into()),
            ..Default::default()
        };
        let (_, v) =
            body_json(list_operations(State(state.clone()), user("u1"), Query(q)).await).await;
        assert_eq!(v["total"], 1);
        assert_eq!(v["items"][0]["operationId"], op1);

        let (_, v) = body_json(
            list_operations(
                State(state.clone()),
                admin(),
                Query(OperationQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(v["total"], 2);

        // Export uses the same filter as the list.
        let export_q = |kind: &str, fmt: &str| ExportQuery {
            kind: kind.into(),
            format: Some(fmt.into()),
            sessions: SessionQuery::default(),
            operations: OperationQuery::default(),
        };
        let (st, v) = body_json(
            export(
                State(state.clone()),
                user("u1"),
                Query(export_q("operations", "json")),
            )
            .await,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["total"], 1);
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["truncated"], false);

        let resp = export(
            State(state.clone()),
            admin(),
            Query(export_q("sessions", "csv")),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        let csv = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(csv.lines().count(), 3, "{}", csv);
        assert!(csv.contains(",u1,u1,"));
        assert!(csv.contains(",u2,u2,"));

        // Exports are themselves audited.
        let access = audit_events::events_after(&state.db, ACCESS_STREAM, 0, 10).unwrap();
        assert_eq!(access.len(), 2);
        assert_eq!(access[0].event_type, "audit.export");
        assert_eq!(access[0].payload["userId"], "u1");
        assert_eq!(access[1].payload["admin"], true);
    }

    #[tokio::test]
    async fn system_events_are_admin_only_and_include_the_current_start() {
        let state = test_state().await;
        let resp = list_system_events(
            State(state.clone()),
            user("u1"),
            Query(SystemQuery::default()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let (st, v) = body_json(
            list_system_events(State(state.clone()), admin(), Query(SystemQuery::default())).await,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["items"][0]["eventType"], "system.startup");
        assert_eq!(
            v["current"]["previousShutdownClean"],
            serde_json::Value::Null
        );
        assert_eq!(v["current"]["sessionsClosed"], 0);
    }

    #[tokio::test]
    async fn integrity_check_is_admin_only_and_recorded() {
        let state = test_state().await;
        let resp = integrity(
            State(state.clone()),
            user("u1"),
            Query(IntegrityQuery::default()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let (st, v) = body_json(
            integrity(
                State(state.clone()),
                admin(),
                Query(IntegrityQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["appendOnlyGuardsPresent"], true);
        assert!(v["limitation"]
            .as_str()
            .unwrap()
            .contains("full control of the host"));
        let access = audit_events::events_after(&state.db, ACCESS_STREAM, 0, 10).unwrap();
        assert!(access
            .iter()
            .any(|e| e.event_type == "audit.integrity_check"));
    }

    #[tokio::test]
    async fn recording_reads_are_owner_scoped_and_audited() {
        let state = test_state().await;
        seed_session(&state, "s1", "u1");
        let store = state.recordings.clone().unwrap();
        let rec = store.start("s1", 80, 24).unwrap();
        rec.output(b"hello");
        let rid = rec.recording_id().to_string();
        assert_eq!(rec.finish().await, Integrity::Complete);

        let (st, _) = body_json(
            recording_events(
                State(state.clone()),
                user("u2"),
                Path(rid.clone()),
                Query(EventsQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        let (st, v) = body_json(
            recording_events(
                State(state.clone()),
                user("u1"),
                Path(rid.clone()),
                Query(EventsQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["events"].as_array().unwrap().len(), 2);
        assert_eq!(v["events"][1]["k"], "o");
        assert_eq!(v["problems"].as_array().unwrap().len(), 0);
        assert_eq!(v["recording"]["recordingId"], rid);

        let (_, v) =
            body_json(list_recordings(State(state.clone()), user("u1"), Path("s1".into())).await)
                .await;
        assert_eq!(v["items"][0]["chunks"].as_array().unwrap().len(), 1);

        let access = audit_events::events_after(&state.db, ACCESS_STREAM, 0, 10).unwrap();
        assert_eq!(access.len(), 1);
        assert_eq!(access[0].event_type, "audit.recording_read");
        assert_eq!(access[0].payload["recordingId"], rid);
    }

    #[test]
    fn csv_quotes_and_neutralises_formulas() {
        assert_eq!(csv_field(&serde_json::json!("plain")), "plain");
        assert_eq!(csv_field(&serde_json::json!("a,b")), "\"a,b\"");
        assert_eq!(
            csv_field(&serde_json::json!("say \"hi\"")),
            "\"say \"\"hi\"\"\""
        );
        assert_eq!(csv_field(&serde_json::json!("=1+1")), "'=1+1");
        assert_eq!(csv_field(&serde_json::json!(null)), "");
        assert_eq!(
            csv_field(&serde_json::json!({"kind": "gap"})),
            "\"{\"\"kind\"\":\"\"gap\"\"}\""
        );
    }

    #[test]
    fn csv_follows_nested_paths() {
        let rows = vec![serde_json::json!({
            "sessionId": "s1",
            "actor": {"user_id": "u1", "username": "alice"},
            "target": {"server_id": "srv"},
            "integrity": {"kind": "complete"},
            "source": "terminal",
        })];
        let csv = to_csv(SESSION_COLUMNS, &rows);
        let mut lines = csv.lines();
        assert!(lines.next().unwrap().starts_with("sessionId,connectedAt,"));
        let row = lines.next().unwrap();
        assert!(row.starts_with("s1,,,,u1,alice,,terminal,srv,"));
        assert!(row.ends_with(",complete"));
    }
}
