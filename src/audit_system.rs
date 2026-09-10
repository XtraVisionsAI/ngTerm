//! Process-level audit events (AUD-08): every start and clean shutdown of
//! the server is recorded on the `system` stream, together with what had to
//! be recovered. A start that follows a start without a shutdown in between
//! means the previous process died unexpectedly; the sessions, operations
//! and recordings it left open are closed as interrupted, never left
//! looking live or complete.

use serde::Serialize;
use serde_json::json;

use crate::audit_events::{self, Integrity, NewEvent};
use crate::db::Database;

pub const SYSTEM_STREAM: &str = "system";
pub const STARTUP_EVENT: &str = "system.startup";
pub const SHUTDOWN_EVENT: &str = "system.shutdown";

const RESTART_REASON: &str = "server_restart";
const SHUTDOWN_REASON: &str = "server_shutdown";

/// What startup found and fixed. Exposed through `/api/health` so an
/// unclean previous shutdown is visible without reading the audit store.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupReport {
    pub started_at: String,
    /// `None` on the very first start (nothing to compare against).
    pub previous_shutdown_clean: Option<bool>,
    pub sessions_closed: usize,
    pub operations_interrupted: usize,
    pub recordings_interrupted: usize,
    pub recording_enabled: bool,
}

/// Recover whatever the previous process left open and record the start.
/// Recovery runs even when the event cannot be written; the error is then
/// returned so the caller can log it.
pub fn startup(
    db: &Database,
    recordings_interrupted: usize,
    recording_enabled: bool,
) -> Result<StartupReport, (StartupReport, String)> {
    let previous_shutdown_clean = match audit_events::recent_events(db, SYSTEM_STREAM, 1) {
        Ok(events) => events.first().map(|e| e.event_type == SHUTDOWN_EVENT),
        Err(e) => {
            tracing::error!("Could not read the system audit stream: {}", e);
            None
        }
    };
    if previous_shutdown_clean == Some(false) {
        tracing::warn!("Previous server process did not shut down cleanly; recovering audit state");
    }
    let sessions_closed = audit_events::close_open_sessions(
        db,
        RESTART_REASON,
        "server restarted while the session was open",
    )
    .unwrap_or_else(|e| {
        tracing::error!("Could not close stale audit sessions: {}", e);
        0
    });
    let operations_interrupted = audit_events::interrupt_open_operations(
        db,
        "server restarted before the operation reported completion",
    )
    .unwrap_or_else(|e| {
        tracing::error!("Could not interrupt stale audit operations: {}", e);
        0
    });
    if sessions_closed + operations_interrupted > 0 {
        tracing::warn!(
            "Recovered audit state: {} session(s) closed, {} operation(s) interrupted",
            sessions_closed,
            operations_interrupted
        );
    }
    let report = StartupReport {
        started_at: chrono::Utc::now().to_rfc3339(),
        previous_shutdown_clean,
        sessions_closed,
        operations_interrupted,
        recordings_interrupted,
        recording_enabled,
    };
    let payload = serde_json::to_value(&report).unwrap_or_else(|_| json!({}));
    match audit_events::append_event(
        db,
        NewEvent {
            stream_id: SYSTEM_STREAM,
            occurred_at: None,
            session_id: None,
            operation_id: None,
            event_type: STARTUP_EVENT,
            payload,
            integrity: Integrity::Complete,
        },
    ) {
        Ok(_) => Ok(report),
        Err(e) => Err((report, e)),
    }
}

/// Close anything still open at a clean shutdown and record the shutdown.
/// `sessions_requested` / `sessions_unconfirmed` describe the drain.
pub fn shutdown(
    db: &Database,
    sessions_requested: usize,
    sessions_unconfirmed: usize,
) -> Result<(), String> {
    let sessions_closed = audit_events::close_open_sessions(
        db,
        SHUTDOWN_REASON,
        "server shut down before the session confirmed its end",
    )?;
    let operations_interrupted = audit_events::interrupt_open_operations(
        db,
        "server shut down before the operation reported completion",
    )?;
    audit_events::append_event(
        db,
        NewEvent {
            stream_id: SYSTEM_STREAM,
            occurred_at: None,
            session_id: None,
            operation_id: None,
            event_type: SHUTDOWN_EVENT,
            payload: json!({
                "sessionsRequested": sessions_requested,
                "sessionsUnconfirmed": sessions_unconfirmed,
                "sessionsClosed": sessions_closed,
                "operationsInterrupted": operations_interrupted,
            }),
            integrity: Integrity::Complete,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_events::{
        Actor, ActorKind, AuditSession, OperationIntent, OperationKind, OperationStatus, Source,
        Target,
    };

    fn temp_db() -> Database {
        let dir = std::env::temp_dir().join(format!("ngterm-audsys-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Database::open(dir.to_str().unwrap()).unwrap()
    }

    fn open_session_with_running_op(db: &Database, sid: &str) -> String {
        audit_events::session_started(
            db,
            &AuditSession {
                session_id: sid.into(),
                actor: Actor {
                    kind: Some(ActorKind::Human),
                    user_id: Some("u1".into()),
                    ..Default::default()
                },
                target: Target::default(),
                source: Source::Terminal,
                parent_session_id: None,
                connected_at: chrono::Utc::now().to_rfc3339(),
                disconnected_at: None,
                disconnect_reason: None,
                integrity: Integrity::Complete,
            },
        )
        .unwrap();
        let op = audit_events::operation_intended(
            db,
            &OperationIntent {
                session_id: Some(sid.into()),
                task_id: None,
                parent_operation_id: None,
                actor: Actor::default(),
                source: Source::Terminal,
                kind: OperationKind::Command,
                summary: "sleep 999".into(),
                target: Target::default(),
                cwd: None,
            },
        )
        .unwrap();
        audit_events::operation_started(db, &op).unwrap();
        op
    }

    #[test]
    fn crash_recovery_closes_what_the_dead_process_left_open() {
        let db = temp_db();
        let op = open_session_with_running_op(&db, "s1");

        // First start ever: nothing to compare against, nothing stale... except
        // the rows we planted to simulate a crash.
        let first = startup(&db, 2, true).unwrap();
        assert_eq!(first.previous_shutdown_clean, None);
        assert_eq!(first.sessions_closed, 1);
        assert_eq!(first.operations_interrupted, 1);
        assert_eq!(first.recordings_interrupted, 2);

        let session = audit_events::get_session(&db, "s1").unwrap().unwrap();
        assert!(session.disconnected_at.is_some());
        assert_eq!(session.disconnect_reason.as_deref(), Some("server_restart"));
        assert!(matches!(session.integrity, Integrity::Truncated { .. }));
        let op = audit_events::get_operation(&db, &op).unwrap().unwrap();
        assert_eq!(op.status, OperationStatus::Interrupted);

        // Second start without a shutdown in between: the previous run died.
        let second = startup(&db, 0, true).unwrap();
        assert_eq!(second.previous_shutdown_clean, Some(false));
        assert_eq!(second.sessions_closed, 0);

        // Clean shutdown, then start: recognised as clean.
        open_session_with_running_op(&db, "s2");
        shutdown(&db, 1, 1).unwrap();
        let s2 = audit_events::get_session(&db, "s2").unwrap().unwrap();
        assert_eq!(s2.disconnect_reason.as_deref(), Some("server_shutdown"));
        let third = startup(&db, 0, false).unwrap();
        assert_eq!(third.previous_shutdown_clean, Some(true));
        assert!(!third.recording_enabled);

        let events = audit_events::recent_events(&db, SYSTEM_STREAM, 10).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
        assert_eq!(
            kinds,
            vec![STARTUP_EVENT, SHUTDOWN_EVENT, STARTUP_EVENT, STARTUP_EVENT]
        );
        assert_eq!(events[1].payload["sessionsClosed"], 1);
        assert_eq!(events[1].payload["operationsInterrupted"], 1);
        assert_eq!(events[3].payload["sessionsClosed"], 1);
    }

    #[test]
    fn recovery_still_happens_when_the_event_cannot_be_written() {
        let db = temp_db();
        let op = open_session_with_running_op(&db, "s1");
        db.conn().execute_batch("DROP TABLE audit_events").unwrap();
        let err = startup(&db, 0, true).unwrap_err();
        assert_eq!(err.0.sessions_closed, 1);
        assert_eq!(
            audit_events::get_operation(&db, &op)
                .unwrap()
                .unwrap()
                .status,
            OperationStatus::Interrupted
        );
    }
}
