//! Versioned audit event model (AUD-01).
//!
//! Three linked records describe everything that happens on the platform:
//!
//! * an **audit session** — a terminal/SFTP session a user opened against a
//!   target; it owns the timeline and knows whether recording was complete;
//! * an **operation** — one thing somebody or something tried to do (a
//!   command, a file write, a tool call). It is written *before* execution as
//!   an intent and finished afterwards with a status and evidence;
//! * an **event** — an ordered fact inside a stream (session or task), with
//!   a schema version, monotonically increasing sequence and integrity flag.
//!
//! What is *not* known is stated, never guessed: an exit code is either
//! reported by the executor, inferred from parsing terminal output, or
//! unknown with a reason.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::Database;

/// Bump when the meaning or shape of stored events changes.
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// Who initiated an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A person typing in a terminal or clicking in the UI.
    Human,
    /// An external CLI (e.g. Claude Code) launched by the platform.
    ExternalTool,
    /// The in-process agent engine acting on the user's behalf.
    EmbeddedAgent,
    /// The platform itself (reaper, scheduler, migrations).
    Platform,
}

/// Through which channel the operation entered the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Terminal,
    Chat,
    Api,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Actor {
    pub kind: Option<ActorKind>,
    pub user_id: Option<String>,
    pub username: Option<String>,
    /// Tool definition the actor ran under (external or native).
    pub tool_id: Option<String>,
    pub agent_id: Option<String>,
    pub remote_addr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Target {
    pub server_id: Option<String>,
    pub server_alias: Option<String>,
    pub server_host: Option<String>,
    /// Account used on the remote host.
    pub remote_user: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Command,
    FileRead,
    FileWrite,
    FileDelete,
    FileRename,
    Mkdir,
    Upload,
    Download,
    Git,
    ToolCall,
    McpCall,
    Approval,
    ConfigChange,
    /// Starting an agent (external CLI or embedded engine) inside a session.
    /// The summary carries the tool and its capability declaration.
    AgentLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// Registered before execution; nothing has been sent yet.
    Intended,
    Running,
    Succeeded,
    Failed,
    Denied,
    TimedOut,
    Cancelled,
    /// Execution started but its end was never observed (process died,
    /// connection dropped, recorder lagged).
    Interrupted,
    /// Outcome cannot be determined; see `exit.reason`.
    Unknown,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, OperationStatus::Intended | OperationStatus::Running)
    }
}

/// How we know the outcome. Parsing terminal output is an inference and is
/// labelled as such; only an executor's own report counts as confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    ExecutorConfirmed,
    ParsedFromOutput,
    /// Stated by the actor (e.g. a CLI's own result message), not verified.
    Declared,
    None,
}

/// Exit status with the reason spelled out when it is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExitStatus {
    Known { code: i64 },
    Unknown { reason: String },
}

/// Whether the recorded stream is trustworthy as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Integrity {
    Complete,
    /// `dropped` items were lost between producer and recorder.
    Gap {
        dropped: u64,
    },
    Truncated {
        reason: String,
    },
}

impl Integrity {
    fn as_db(&self) -> (String, Option<String>) {
        match self {
            Integrity::Complete => ("complete".into(), None),
            Integrity::Gap { dropped } => ("gap".into(), Some(dropped.to_string())),
            Integrity::Truncated { reason } => ("truncated".into(), Some(reason.clone())),
        }
    }

    fn from_db(kind: &str, detail: Option<String>) -> Self {
        match kind {
            "gap" => Integrity::Gap {
                dropped: detail.and_then(|d| d.parse().ok()).unwrap_or(0),
            },
            "truncated" => Integrity::Truncated {
                reason: detail.unwrap_or_default(),
            },
            _ => Integrity::Complete,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditSession {
    pub session_id: String,
    pub actor: Actor,
    pub target: Target,
    pub source: Source,
    pub parent_session_id: Option<String>,
    pub connected_at: String,
    pub disconnected_at: Option<String>,
    pub disconnect_reason: Option<String>,
    pub integrity: Integrity,
}

/// Registration of an operation before it runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationIntent {
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub parent_operation_id: Option<String>,
    pub actor: Actor,
    pub source: Source,
    pub kind: OperationKind,
    /// Human-readable, already redacted description (command text, path…).
    pub summary: String,
    pub target: Target,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub operation_id: String,
    pub parent_operation_id: Option<String>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub actor: Actor,
    pub source: Source,
    pub kind: OperationKind,
    pub summary: String,
    pub target: Target,
    pub cwd: Option<String>,
    pub status: OperationStatus,
    pub exit: Option<ExitStatus>,
    pub evidence: Evidence,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// Terminal outcome of an operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationOutcome {
    pub status: OperationStatus,
    pub exit: ExitStatus,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub stream_id: String,
    pub seq: i64,
    pub occurred_at: String,
    pub recorded_at: String,
    pub session_id: Option<String>,
    pub operation_id: Option<String>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub integrity: Integrity,
}

/// What a caller supplies to append an event; ids, seq and timestamps are
/// assigned by the store.
#[derive(Debug, Clone)]
pub struct NewEvent<'a> {
    pub stream_id: &'a str,
    pub occurred_at: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub operation_id: Option<&'a str>,
    pub event_type: &'a str,
    pub payload: serde_json::Value,
    pub integrity: Integrity,
}

/// Filter for operation listings. `user_id` is mandatory for non-admins and
/// enforced by the caller.
#[derive(Debug, Default, Clone)]
pub struct OperationFilter {
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub actor_kind: Option<ActorKind>,
    pub status: Option<OperationStatus>,
    pub server_id: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    pub summary_contains: Option<String>,
    pub kind: Option<OperationKind>,
    /// Only operations caused by this one (lower-level calls of a tool call).
    pub parent_operation_id: Option<String>,
}

/// Filter for session listings. `user_id` is mandatory for non-admins and
/// enforced by the caller.
#[derive(Debug, Default, Clone)]
pub struct SessionFilter {
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub server_id: Option<String>,
    pub remote_user: Option<String>,
    pub source: Option<Source>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
    /// `Some(true)` = still connected, `Some(false)` = ended.
    pub active: Option<bool>,
    /// `complete`, `gap` or `truncated`.
    pub integrity: Option<String>,
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

fn from_json<T: for<'de> Deserialize<'de> + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn enum_from<T: for<'de> Deserialize<'de>>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// Redact obvious secrets from free text before it is stored or indexed.
/// This is a safety net for command lines and paths, not a guarantee.
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    let patterns: &[(&str, &str)] = &[
        (r"(?i)(authorization:\s*bearer\s+)\S+", "${1}[REDACTED]"),
        (
            r"(?i)((?:password|passwd|pwd|token|secret|api[_-]?key)\s*[=:]\s*)\S+",
            "${1}[REDACTED]",
        ),
        (
            r"(?i)(--?(?:password|token|api-key|secret)[= ])\S+",
            "${1}[REDACTED]",
        ),
        (r"\bsk-[A-Za-z0-9_-]{8,}", "[REDACTED]"),
        (r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED]"),
        (r"(?i)(https?://[^/\s:]+:)[^@\s]+@", "${1}[REDACTED]@"),
    ];
    for (re, rep) in patterns {
        if let Ok(re) = regex::Regex::new(re) {
            out = re.replace_all(&out, *rep).to_string();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Migration v2: the versioned audit tables. `audit_logs` is kept untouched
/// for the historical connection list.
pub fn migration_v2_audit_events(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS audit_sessions (
            session_id          TEXT PRIMARY KEY,
            user_id             TEXT,
            username            TEXT,
            actor_kind          TEXT,
            remote_addr         TEXT,
            source              TEXT NOT NULL,
            server_id           TEXT,
            server_alias        TEXT,
            server_host         TEXT,
            remote_user         TEXT,
            parent_session_id   TEXT,
            connected_at        TEXT NOT NULL,
            disconnected_at     TEXT,
            disconnect_reason   TEXT,
            integrity           TEXT NOT NULL DEFAULT 'complete',
            integrity_detail    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_audit_sessions_user ON audit_sessions(user_id, connected_at);

        CREATE TABLE IF NOT EXISTS audit_operations (
            operation_id        TEXT PRIMARY KEY,
            parent_operation_id TEXT,
            session_id          TEXT,
            task_id             TEXT,
            actor_json          TEXT NOT NULL,
            actor_kind          TEXT,
            user_id             TEXT,
            source              TEXT NOT NULL,
            kind                TEXT NOT NULL,
            summary             TEXT NOT NULL,
            target_json         TEXT NOT NULL,
            server_id           TEXT,
            cwd                 TEXT,
            status              TEXT NOT NULL,
            exit_json           TEXT,
            evidence            TEXT NOT NULL DEFAULT 'none',
            started_at          TEXT NOT NULL,
            finished_at         TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_audit_ops_user_time ON audit_operations(user_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_audit_ops_session ON audit_operations(session_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_audit_ops_task ON audit_operations(task_id);

        CREATE TABLE IF NOT EXISTS audit_events (
            event_id        TEXT PRIMARY KEY,
            schema_version  INTEGER NOT NULL,
            stream_id       TEXT NOT NULL,
            seq             INTEGER NOT NULL,
            occurred_at     TEXT NOT NULL,
            recorded_at     TEXT NOT NULL,
            session_id      TEXT,
            operation_id    TEXT,
            event_type      TEXT NOT NULL,
            payload         TEXT NOT NULL,
            integrity       TEXT NOT NULL DEFAULT 'complete',
            integrity_detail TEXT,
            UNIQUE(stream_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_audit_events_op ON audit_events(operation_id);
        ",
    )
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub fn session_started(db: &Database, session: &AuditSession) -> Result<(), String> {
    let (integ, detail) = session.integrity.as_db();
    db.conn()
        .execute(
            "INSERT OR REPLACE INTO audit_sessions (session_id, user_id, username, actor_kind, remote_addr, source, server_id, server_alias, server_host, remote_user, parent_session_id, connected_at, disconnected_at, disconnect_reason, integrity, integrity_detail)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,NULL,?13,?14)",
            params![
                session.session_id,
                session.actor.user_id,
                session.actor.username,
                session.actor.kind.map(|k| enum_str(&k)),
                session.actor.remote_addr,
                enum_str(&session.source),
                session.target.server_id,
                session.target.server_alias,
                session.target.server_host,
                session.target.remote_user,
                session.parent_session_id,
                session.connected_at,
                integ,
                detail,
            ],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Close a session. Also marks every operation of the session that never
/// reported an end as `interrupted`, with the reason recorded, so nothing
/// stays "running" forever.
pub fn session_ended(
    db: &Database,
    session_id: &str,
    reason: &str,
    integrity: Integrity,
) -> Result<(), String> {
    let (integ, detail) = integrity.as_db();
    let ts = now();
    let conn = db.conn();
    conn.execute(
        "UPDATE audit_sessions SET disconnected_at = ?1, disconnect_reason = ?2, integrity = ?3, integrity_detail = ?4 WHERE session_id = ?5 AND disconnected_at IS NULL",
        params![ts, reason, integ, detail, session_id],
    )
    .map_err(|e| e.to_string())?;
    let exit = to_json(&ExitStatus::Unknown {
        reason: format!(
            "session ended ({}) before the operation reported completion",
            reason
        ),
    });
    conn.execute(
        "UPDATE audit_operations SET status = 'interrupted', exit_json = ?1, finished_at = ?2 WHERE session_id = ?3 AND status IN ('intended','running')",
        params![exit, ts, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

const SESSION_COLUMNS: &str = "session_id, user_id, username, actor_kind, remote_addr, source, server_id, server_alias, server_host, remote_user, parent_session_id, connected_at, disconnected_at, disconnect_reason, integrity, integrity_detail";

fn row_to_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditSession> {
    Ok(AuditSession {
        session_id: r.get(0)?,
        actor: Actor {
            user_id: r.get(1)?,
            username: r.get(2)?,
            kind: r.get::<_, Option<String>>(3)?.and_then(|s| enum_from(&s)),
            remote_addr: r.get(4)?,
            ..Default::default()
        },
        source: enum_from(&r.get::<_, String>(5)?).unwrap_or(Source::Api),
        target: Target {
            server_id: r.get(6)?,
            server_alias: r.get(7)?,
            server_host: r.get(8)?,
            remote_user: r.get(9)?,
        },
        parent_session_id: r.get(10)?,
        connected_at: r.get(11)?,
        disconnected_at: r.get(12)?,
        disconnect_reason: r.get(13)?,
        integrity: Integrity::from_db(&r.get::<_, String>(14)?, r.get(15)?),
    })
}

pub fn get_session(db: &Database, session_id: &str) -> Result<Option<AuditSession>, String> {
    db.conn()
        .query_row(
            &format!(
                "SELECT {} FROM audit_sessions WHERE session_id = ?1",
                SESSION_COLUMNS
            ),
            params![session_id],
            row_to_session,
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.to_string()),
        })
}

/// Page of sessions newest first, plus the total matching the filter so
/// the same query drives both the list and its export.
pub fn list_sessions(
    db: &Database,
    filter: &SessionFilter,
    limit: u32,
    offset: u32,
) -> Result<(Vec<AuditSession>, u32), String> {
    let mut conds = Vec::new();
    let mut ps: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut add = |cond: &str, v: Box<dyn rusqlite::types::ToSql>| {
        ps.push(v);
        conds.push(cond.replace('?', &format!("?{}", ps.len())));
    };
    if let Some(v) = &filter.user_id {
        add("user_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.username {
        add("username = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.server_id {
        add("server_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.remote_user {
        add("remote_user = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.source {
        add("source = ?", Box::new(enum_str(v)));
    }
    if let Some(v) = &filter.time_from {
        add("connected_at >= ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.time_to {
        add("connected_at <= ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.integrity {
        add("integrity = ?", Box::new(v.clone()));
    }
    match filter.active {
        Some(true) => conds.push("disconnected_at IS NULL".to_string()),
        Some(false) => conds.push("disconnected_at IS NOT NULL".to_string()),
        None => {}
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };

    let conn = db.conn();
    let refs: Vec<&dyn rusqlite::types::ToSql> = ps.iter().map(|p| p.as_ref()).collect();
    let total: u32 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM audit_sessions {}", where_clause),
            refs.as_slice(),
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    ps.push(Box::new(limit));
    let li = ps.len();
    ps.push(Box::new(offset));
    let oi = ps.len();
    let refs: Vec<&dyn rusqlite::types::ToSql> = ps.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM audit_sessions {} ORDER BY connected_at DESC LIMIT ?{} OFFSET ?{}",
            SESSION_COLUMNS, where_clause, li, oi
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(refs.as_slice(), row_to_session)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok((rows, total))
}

/// Register an operation *before* it executes. Returns the operation id the
/// caller must use to report the outcome.
pub fn operation_intended(db: &Database, intent: &OperationIntent) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let summary = redact(&intent.summary);
    db.conn()
        .execute(
            "INSERT INTO audit_operations (operation_id, parent_operation_id, session_id, task_id, actor_json, actor_kind, user_id, source, kind, summary, target_json, server_id, cwd, status, exit_json, evidence, started_at, finished_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'intended',NULL,'none',?14,NULL)",
            params![
                id,
                intent.parent_operation_id,
                intent.session_id,
                intent.task_id,
                to_json(&intent.actor),
                intent.actor.kind.map(|k| enum_str(&k)),
                intent.actor.user_id,
                enum_str(&intent.source),
                enum_str(&intent.kind),
                summary,
                to_json(&intent.target),
                intent.target.server_id,
                intent.cwd,
                now(),
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

pub fn operation_started(db: &Database, operation_id: &str) -> Result<(), String> {
    db.conn()
        .execute(
            "UPDATE audit_operations SET status = 'running' WHERE operation_id = ?1 AND status = 'intended'",
            params![operation_id],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Record the outcome. A non-terminal status is rejected; an outcome must
/// say how it knows what it knows.
pub fn operation_finished(
    db: &Database,
    operation_id: &str,
    outcome: &OperationOutcome,
) -> Result<(), String> {
    if !outcome.status.is_terminal() {
        return Err(format!(
            "operation outcome must be terminal, got {:?}",
            outcome.status
        ));
    }
    if matches!(outcome.exit, ExitStatus::Known { .. })
        && matches!(outcome.evidence, Evidence::None)
    {
        return Err(
            "a known exit code needs evidence (executor_confirmed, parsed_from_output or declared)"
                .into(),
        );
    }
    let n = db
        .conn()
        .execute(
            "UPDATE audit_operations SET status = ?1, exit_json = ?2, evidence = ?3, finished_at = ?4 WHERE operation_id = ?5",
            params![
                enum_str(&outcome.status),
                to_json(&outcome.exit),
                enum_str(&outcome.evidence),
                now(),
                operation_id
            ],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("unknown operation {}", operation_id));
    }
    Ok(())
}

fn row_to_operation(r: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    Ok(OperationRecord {
        operation_id: r.get(0)?,
        parent_operation_id: r.get(1)?,
        session_id: r.get(2)?,
        task_id: r.get(3)?,
        actor: from_json(&r.get::<_, String>(4)?),
        source: enum_from(&r.get::<_, String>(5)?).unwrap_or(Source::Api),
        kind: enum_from(&r.get::<_, String>(6)?).unwrap_or(OperationKind::Command),
        summary: r.get(7)?,
        target: from_json(&r.get::<_, String>(8)?),
        cwd: r.get(9)?,
        status: enum_from(&r.get::<_, String>(10)?).unwrap_or(OperationStatus::Unknown),
        exit: r
            .get::<_, Option<String>>(11)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        evidence: enum_from(&r.get::<_, String>(12)?).unwrap_or(Evidence::None),
        started_at: r.get(13)?,
        finished_at: r.get(14)?,
    })
}

const OP_COLUMNS: &str = "operation_id, parent_operation_id, session_id, task_id, actor_json, source, kind, summary, target_json, cwd, status, exit_json, evidence, started_at, finished_at";

pub fn get_operation(db: &Database, operation_id: &str) -> Result<Option<OperationRecord>, String> {
    db.conn()
        .query_row(
            &format!(
                "SELECT {} FROM audit_operations WHERE operation_id = ?1",
                OP_COLUMNS
            ),
            params![operation_id],
            row_to_operation,
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.to_string()),
        })
}

pub fn list_operations(
    db: &Database,
    filter: &OperationFilter,
    limit: u32,
    offset: u32,
) -> Result<(Vec<OperationRecord>, u32), String> {
    let mut conds = Vec::new();
    let mut ps: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut add = |cond: &str, v: Box<dyn rusqlite::types::ToSql>| {
        ps.push(v);
        conds.push(cond.replace('?', &format!("?{}", ps.len())));
    };
    if let Some(v) = &filter.user_id {
        add("user_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.session_id {
        add("session_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.task_id {
        add("task_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.actor_kind {
        add("actor_kind = ?", Box::new(enum_str(v)));
    }
    if let Some(v) = &filter.status {
        add("status = ?", Box::new(enum_str(v)));
    }
    if let Some(v) = &filter.kind {
        add("kind = ?", Box::new(enum_str(v)));
    }
    if let Some(v) = &filter.server_id {
        add("server_id = ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.time_from {
        add("started_at >= ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.time_to {
        add("started_at <= ?", Box::new(v.clone()));
    }
    if let Some(v) = &filter.summary_contains {
        add("summary LIKE ?", Box::new(format!("%{}%", v)));
    }
    if let Some(v) = &filter.parent_operation_id {
        add("parent_operation_id = ?", Box::new(v.clone()));
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };

    let conn = db.conn();
    let refs: Vec<&dyn rusqlite::types::ToSql> = ps.iter().map(|p| p.as_ref()).collect();
    let total: u32 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM audit_operations {}", where_clause),
            refs.as_slice(),
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    ps.push(Box::new(limit));
    let li = ps.len();
    ps.push(Box::new(offset));
    let oi = ps.len();
    let refs: Vec<&dyn rusqlite::types::ToSql> = ps.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM audit_operations {} ORDER BY started_at DESC LIMIT ?{} OFFSET ?{}",
            OP_COLUMNS, where_clause, li, oi
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(refs.as_slice(), row_to_operation)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok((rows, total))
}

/// Append an event to a stream. The sequence number is assigned inside a
/// transaction so concurrent producers on the same stream never collide.
pub fn append_event(db: &Database, ev: NewEvent<'_>) -> Result<AuditEvent, String> {
    let mut conn = db.conn();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM audit_events WHERE stream_id = ?1",
            params![ev.stream_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let recorded_at = now();
    let occurred_at = ev
        .occurred_at
        .map(str::to_string)
        .unwrap_or_else(|| recorded_at.clone());
    let event_id = uuid::Uuid::new_v4().to_string();
    let payload = redact_json(ev.payload);
    let (integ, detail) = ev.integrity.as_db();
    tx.execute(
        "INSERT INTO audit_events (event_id, schema_version, stream_id, seq, occurred_at, recorded_at, session_id, operation_id, event_type, payload, integrity, integrity_detail)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            event_id,
            AUDIT_SCHEMA_VERSION,
            ev.stream_id,
            seq,
            occurred_at,
            recorded_at,
            ev.session_id,
            ev.operation_id,
            ev.event_type,
            to_json(&payload),
            integ,
            detail
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(AuditEvent {
        schema_version: AUDIT_SCHEMA_VERSION,
        event_id,
        stream_id: ev.stream_id.to_string(),
        seq,
        occurred_at,
        recorded_at,
        session_id: ev.session_id.map(str::to_string),
        operation_id: ev.operation_id.map(str::to_string),
        event_type: ev.event_type.to_string(),
        payload,
        integrity: ev.integrity,
    })
}

pub(crate) fn redact_json(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(redact(&s)),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(redact_json).collect())
        }
        serde_json::Value::Object(o) => serde_json::Value::Object(
            o.into_iter()
                .map(|(k, v)| {
                    let lk = k.to_ascii_lowercase();
                    let sensitive_key = lk.contains("password")
                        || lk.contains("secret")
                        || lk.contains("token")
                        || lk.contains("api_key")
                        || lk.contains("apikey");
                    // Booleans under such keys are flags ("secret": true), not
                    // secrets; masking them would destroy the information.
                    if sensitive_key && !v.is_boolean() {
                        (k, serde_json::Value::String("[REDACTED]".into()))
                    } else {
                        (k, redact_json(v))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

const EVENT_COLUMNS: &str = "event_id, schema_version, stream_id, seq, occurred_at, recorded_at, session_id, operation_id, event_type, payload, integrity, integrity_detail";

/// Close every audit session still open (server restart or shutdown). The
/// sessions' streams stopped without a confirmed end, so they are marked
/// truncated with `detail`, and their unfinished operations interrupted.
pub fn close_open_sessions(db: &Database, reason: &str, detail: &str) -> Result<usize, String> {
    let ts = now();
    db.conn()
        .execute(
            "UPDATE audit_sessions SET disconnected_at = ?1, disconnect_reason = ?2, integrity = 'truncated', integrity_detail = ?3 WHERE disconnected_at IS NULL",
            params![ts, reason, detail],
        )
        .map_err(|e| e.to_string())
}

/// Mark every operation still `intended` or `running` as interrupted: the
/// process that was executing it is gone, so its outcome is unknown.
pub fn interrupt_open_operations(db: &Database, reason: &str) -> Result<usize, String> {
    let exit = to_json(&ExitStatus::Unknown {
        reason: reason.to_string(),
    });
    db.conn()
        .execute(
            "UPDATE audit_operations SET status = 'interrupted', exit_json = ?1, finished_at = ?2 WHERE status IN ('intended','running')",
            params![exit, now()],
        )
        .map_err(|e| e.to_string())
}

/// The most recent events of a stream, newest first.
pub fn recent_events(
    db: &Database,
    stream_id: &str,
    limit: u32,
) -> Result<Vec<AuditEvent>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM audit_events WHERE stream_id = ?1 ORDER BY seq DESC LIMIT ?2",
            EVENT_COLUMNS
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![stream_id, limit], row_to_event)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Events of a stream after `after_seq`, in order. Gaps in `seq` are not
/// expected (assignment is transactional) but the caller can detect them.
pub fn events_after(
    db: &Database,
    stream_id: &str,
    after_seq: i64,
    limit: u32,
) -> Result<Vec<AuditEvent>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM audit_events WHERE stream_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            EVENT_COLUMNS
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![stream_id, after_seq, limit], row_to_event)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// All events linked to one operation, in recording order.
pub fn events_for_operation(db: &Database, operation_id: &str) -> Result<Vec<AuditEvent>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT event_id, schema_version, stream_id, seq, occurred_at, recorded_at, session_id, operation_id, event_type, payload, integrity, integrity_detail FROM audit_events WHERE operation_id = ?1 ORDER BY recorded_at ASC, seq ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![operation_id], row_to_event)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
    Ok(AuditEvent {
        event_id: r.get(0)?,
        schema_version: r.get::<_, i64>(1)? as u32,
        stream_id: r.get(2)?,
        seq: r.get(3)?,
        occurred_at: r.get(4)?,
        recorded_at: r.get(5)?,
        session_id: r.get(6)?,
        operation_id: r.get(7)?,
        event_type: r.get(8)?,
        payload: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or(serde_json::Value::Null),
        integrity: Integrity::from_db(&r.get::<_, String>(10)?, r.get(11)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Database, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ngterm-aud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Database::open(dir.to_str().unwrap()).unwrap(), dir)
    }

    fn human(user: &str) -> Actor {
        Actor {
            kind: Some(ActorKind::Human),
            user_id: Some(user.into()),
            username: Some(user.into()),
            remote_addr: Some("10.0.0.5".into()),
            ..Default::default()
        }
    }

    fn intent(session: &str, actor: Actor, summary: &str) -> OperationIntent {
        OperationIntent {
            session_id: Some(session.into()),
            task_id: None,
            parent_operation_id: None,
            actor,
            source: Source::Terminal,
            kind: OperationKind::Command,
            summary: summary.into(),
            target: Target {
                server_id: Some("srv1".into()),
                server_alias: Some("web-1".into()),
                server_host: Some("10.1.1.1".into()),
                remote_user: Some("deploy".into()),
            },
            cwd: Some("/srv".into()),
        }
    }

    #[test]
    fn session_operation_and_events_link_and_close_consistently() {
        let (db, dir) = temp_db();
        assert!(db.schema_version() >= 2);

        session_started(
            &db,
            &AuditSession {
                session_id: "s1".into(),
                actor: human("alice"),
                target: Target {
                    server_id: Some("srv1".into()),
                    ..Default::default()
                },
                source: Source::Terminal,
                parent_session_id: None,
                connected_at: now(),
                disconnected_at: None,
                disconnect_reason: None,
                integrity: Integrity::Complete,
            },
        )
        .unwrap();

        // Intent is written before execution and carries a redacted summary.
        let op = operation_intended(
            &db,
            &intent(
                "s1",
                human("alice"),
                "mysql -u root --password=hunter2 -e 'select 1'",
            ),
        )
        .unwrap();
        let rec = get_operation(&db, &op).unwrap().unwrap();
        assert_eq!(rec.status, OperationStatus::Intended);
        assert!(rec.summary.contains("[REDACTED]") && !rec.summary.contains("hunter2"));
        operation_started(&db, &op).unwrap();

        // Events get per-stream monotonic sequence numbers and link to the op.
        let e1 = append_event(
            &db,
            NewEvent {
                stream_id: "s1",
                occurred_at: None,
                session_id: Some("s1"),
                operation_id: Some(&op),
                event_type: "command_output",
                payload: serde_json::json!({"chunk": "ok", "token": "abc"}),
                integrity: Integrity::Complete,
            },
        )
        .unwrap();
        let e2 = append_event(
            &db,
            NewEvent {
                stream_id: "s1",
                occurred_at: None,
                session_id: Some("s1"),
                operation_id: Some(&op),
                event_type: "gap",
                payload: serde_json::json!({}),
                integrity: Integrity::Gap { dropped: 3 },
            },
        )
        .unwrap();
        assert_eq!((e1.seq, e2.seq), (1, 2));
        assert_eq!(e1.schema_version, AUDIT_SCHEMA_VERSION);
        assert_eq!(e1.payload["token"], "[REDACTED]");
        let other = append_event(
            &db,
            NewEvent {
                stream_id: "task-9",
                occurred_at: None,
                session_id: None,
                operation_id: None,
                event_type: "task_created",
                payload: serde_json::json!({}),
                integrity: Integrity::Complete,
            },
        )
        .unwrap();
        assert_eq!(other.seq, 1, "streams are numbered independently");
        let replay = events_after(&db, "s1", 0, 10).unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[1].integrity, Integrity::Gap { dropped: 3 });

        // Outcome: a known exit code must state its evidence.
        let err = operation_finished(
            &db,
            &op,
            &OperationOutcome {
                status: OperationStatus::Succeeded,
                exit: ExitStatus::Known { code: 0 },
                evidence: Evidence::None,
            },
        )
        .unwrap_err();
        assert!(err.contains("evidence"));
        operation_finished(
            &db,
            &op,
            &OperationOutcome {
                status: OperationStatus::Succeeded,
                exit: ExitStatus::Known { code: 0 },
                evidence: Evidence::ParsedFromOutput,
            },
        )
        .unwrap();
        let rec = get_operation(&db, &op).unwrap().unwrap();
        assert_eq!(rec.evidence, Evidence::ParsedFromOutput);
        assert!(rec.finished_at.is_some());

        // A second operation never reports; closing the session marks it
        // interrupted with the reason instead of leaving it running.
        let dangling = operation_intended(&db, &intent("s1", human("alice"), "sleep 999")).unwrap();
        operation_started(&db, &dangling).unwrap();
        session_ended(&db, "s1", "ssh_closed", Integrity::Complete).unwrap();
        let rec = get_operation(&db, &dangling).unwrap().unwrap();
        assert_eq!(rec.status, OperationStatus::Interrupted);
        match rec.exit {
            Some(ExitStatus::Unknown { reason }) => assert!(reason.contains("ssh_closed")),
            other => panic!("{:?}", other),
        }
        let s = get_session(&db, "s1").unwrap().unwrap();
        assert_eq!(s.disconnect_reason.as_deref(), Some("ssh_closed"));
        assert_eq!(s.actor.kind, Some(ActorKind::Human));

        // Listing is scoped by user and distinguishes actors.
        let bob_op = operation_intended(
            &db,
            &intent(
                "s2",
                Actor {
                    kind: Some(ActorKind::EmbeddedAgent),
                    user_id: Some("bob".into()),
                    agent_id: Some("agent-s2".into()),
                    ..Default::default()
                },
                "ls",
            ),
        )
        .unwrap();
        let (alice_ops, n) = list_operations(
            &db,
            &OperationFilter {
                user_id: Some("alice".into()),
                ..Default::default()
            },
            50,
            0,
        )
        .unwrap();
        assert_eq!(n, 2);
        assert!(alice_ops.iter().all(|o| o.operation_id != bob_op));
        let (agent_ops, _) = list_operations(
            &db,
            &OperationFilter {
                actor_kind: Some(ActorKind::EmbeddedAgent),
                ..Default::default()
            },
            50,
            0,
        )
        .unwrap();
        assert_eq!(agent_ops.len(), 1);
        assert_eq!(agent_ops[0].actor.agent_id.as_deref(), Some("agent-s2"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn redaction_covers_common_secret_shapes() {
        let s = redact("curl -H 'Authorization: Bearer abc.def' https://user:p4ss@host/x?api_key=XYZ --token 123 sk-abcdefghijklmnop AKIAABCDEFGHIJKLMNOP");
        assert!(!s.contains("abc.def"));
        assert!(!s.contains("p4ss"));
        assert!(!s.contains("XYZ"));
        assert!(!s.contains("sk-abcdefghijklmnop"));
        assert!(!s.contains("AKIAABCDEFGHIJKLMNOP"));
        assert!(s.contains("https://user:[REDACTED]@host"));
        assert_eq!(redact("ls -la /tmp"), "ls -la /tmp");
    }
}
