//! Audit wrapper for operations the platform executes on the user's behalf
//! through its own channels (SFTP file operations, helper-shell git queries).
//! Part of AUD-03: an intent is written before the operation runs, the
//! outcome after; if the intent cannot be written the operation is refused
//! (AUD-08), and a handler that stops before reporting leaves the record
//! `interrupted` instead of `running` forever.

use std::future::Future;

use crate::audit_events::{
    self, Actor, ActorKind, Evidence, ExitStatus, OperationIntent, OperationKind, OperationOutcome,
    OperationStatus, Source, Target,
};
use crate::db::Database;
use crate::guard::{self, GuardAction, GuardDecision, GuardRequest};
use crate::AppState;

/// A registered, running operation. Must be finished explicitly; dropping it
/// unfinished records an interruption.
pub struct ManagedOp {
    db: Database,
    operation_id: String,
    finished: bool,
}

impl ManagedOp {
    pub fn id(&self) -> &str {
        &self.operation_id
    }

    fn finish(mut self, outcome: OperationOutcome) {
        self.finished = true;
        if let Err(e) = audit_events::operation_finished(&self.db, &self.operation_id, &outcome) {
            tracing::error!(
                "Outcome of operation {} could not be recorded: {}",
                self.operation_id,
                e
            );
        }
    }

    /// The executor (SFTP server, helper shell) confirmed completion.
    pub fn succeeded(self) {
        self.finish(OperationOutcome {
            status: OperationStatus::Succeeded,
            exit: ExitStatus::Known { code: 0 },
            evidence: Evidence::ExecutorConfirmed,
        })
    }

    /// The executor reported an error; no exit code exists for SFTP/helper
    /// failures, so the reason is kept and the code left unknown.
    /// The executor reported an exit code: 0 is success, anything else a
    /// failure, both confirmed by the executor.
    pub fn exited(self, code: i64) {
        self.finish(OperationOutcome {
            status: if code == 0 {
                OperationStatus::Succeeded
            } else {
                OperationStatus::Failed
            },
            exit: ExitStatus::Known { code },
            evidence: Evidence::ExecutorConfirmed,
        })
    }

    /// The caller stopped waiting; whether the command ended is not known.
    pub fn timed_out(self, reason: &str) {
        self.finish(OperationOutcome {
            status: OperationStatus::TimedOut,
            exit: ExitStatus::Unknown {
                reason: audit_events::redact(reason),
            },
            evidence: Evidence::None,
        })
    }

    pub fn failed(self, reason: &str) {
        self.finish(OperationOutcome {
            status: OperationStatus::Failed,
            exit: ExitStatus::Unknown {
                reason: audit_events::redact(reason),
            },
            evidence: Evidence::ExecutorConfirmed,
        })
    }
}

impl ManagedOp {
    /// Policy or a person refused the operation; nothing ran.
    pub fn denied(self, reason: &str) {
        self.finish(OperationOutcome {
            status: OperationStatus::Denied,
            exit: ExitStatus::Unknown {
                reason: audit_events::redact(reason),
            },
            evidence: Evidence::None,
        })
    }

    /// The requester stopped it; whether the underlying work stopped on the
    /// target is stated in `reason`, never assumed.
    pub fn cancelled(self, reason: &str) {
        self.finish(OperationOutcome {
            status: OperationStatus::Cancelled,
            exit: ExitStatus::Unknown {
                reason: audit_events::redact(reason),
            },
            evidence: Evidence::None,
        })
    }

    /// The end was not observed (session gone, process died).
    pub fn interrupted(self, reason: &str) {
        self.finish(OperationOutcome {
            status: OperationStatus::Interrupted,
            exit: ExitStatus::Unknown {
                reason: audit_events::redact(reason),
            },
            evidence: Evidence::None,
        })
    }
}

impl Drop for ManagedOp {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let outcome = OperationOutcome {
            status: OperationStatus::Interrupted,
            exit: ExitStatus::Unknown {
                reason: "handler stopped before the outcome was observed (client gone or server shutting down)".into(),
            },
            evidence: Evidence::None,
        };
        if let Err(e) = audit_events::operation_finished(&self.db, &self.operation_id, &outcome) {
            tracing::error!(
                "Interrupted operation {} could not be recorded: {}",
                self.operation_id,
                e
            );
        }
    }
}

/// Register and start an operation on behalf of `user_id` inside
/// `session_id`. Fails when the intent cannot be persisted; callers must not
/// execute in that case.
pub fn begin(
    state: &AppState,
    session_id: &str,
    user_id: &str,
    kind: OperationKind,
    summary: String,
    cwd: Option<String>,
) -> Result<ManagedOp, String> {
    let intent = intent_for(state, session_id, user_id, kind, summary, cwd);
    begin_intent(state, &intent)
}

/// Guarded [`begin`]: the guard decides first, then the intent is recorded
/// and the caller executes and reports through the returned [`ManagedOp`].
pub async fn run_begin(
    state: &AppState,
    session_id: &str,
    user_id: &str,
    kind: OperationKind,
    summary: String,
    cwd: Option<String>,
) -> Result<ManagedOp, OpError> {
    let intent = intent_for(state, session_id, user_id, kind, summary, cwd);
    preflight(state, user_id, &intent).await?;
    begin_intent(state, &intent).map_err(OpError::AuditRefused)
}

/// Register and start an arbitrary intent. Fails when the record cannot be
/// persisted; callers must not execute in that case.
pub fn begin_intent(state: &AppState, intent: &OperationIntent) -> Result<ManagedOp, String> {
    let operation_id = audit_events::operation_intended(&state.db, intent).map_err(|e| {
        format!(
            "audit record could not be written; operation not executed: {}",
            e
        )
    })?;
    audit_events::operation_started(&state.db, &operation_id).map_err(|e| {
        format!(
            "audit record could not be updated; operation not executed: {}",
            e
        )
    })?;
    Ok(ManagedOp {
        db: state.db.clone(),
        operation_id,
        finished: false,
    })
}

/// Build the intent `begin` would record, so the guard judges exactly what
/// will be written.
fn intent_for(
    state: &AppState,
    session_id: &str,
    user_id: &str,
    kind: OperationKind,
    summary: String,
    cwd: Option<String>,
) -> OperationIntent {
    let session = audit_events::get_session(&state.db, session_id)
        .ok()
        .flatten();
    let actor = Actor {
        kind: Some(ActorKind::Human),
        user_id: Some(user_id.to_string()),
        username: session.as_ref().and_then(|s| s.actor.username.clone()),
        remote_addr: session.as_ref().and_then(|s| s.actor.remote_addr.clone()),
        ..Default::default()
    };
    let target = session
        .as_ref()
        .map(|s| s.target.clone())
        .unwrap_or_else(|| Target {
            server_id: state.sessions.get_server_id(session_id),
            ..Default::default()
        });
    OperationIntent {
        session_id: Some(session_id.to_string()),
        task_id: None,
        parent_operation_id: None,
        actor,
        source: Source::Api,
        kind,
        summary,
        target,
        cwd,
    }
}

/// Ask the installed guard about `intent` on behalf of `user_id`. `Ok` means
/// proceed; the error carries the decision for the HTTP answer.
pub async fn preflight(
    state: &AppState,
    user_id: &str,
    intent: &OperationIntent,
) -> Result<(), OpError> {
    let decision = guard::check(
        state.guard(),
        GuardRequest {
            user_id,
            is_admin: user_id == "admin",
            action: GuardAction::Operation { intent },
        },
    )
    .await;
    match decision {
        GuardDecision::Proceed => Ok(()),
        other => Err(OpError::Blocked(other)),
    }
}

/// Run `op` as a managed operation: guard check, intent, the future, then
/// the truthful outcome. The returned error is the guard's decision, the
/// audit refusal or the operation's own error.
pub async fn run<T, F>(
    state: &AppState,
    session_id: &str,
    user_id: &str,
    kind: OperationKind,
    summary: String,
    cwd: Option<String>,
    op: F,
) -> Result<T, OpError>
where
    F: Future<Output = Result<T, String>>,
{
    let intent = intent_for(state, session_id, user_id, kind, summary, cwd);
    preflight(state, user_id, &intent).await?;
    let managed = begin_intent(state, &intent).map_err(OpError::AuditRefused)?;
    match op.await {
        Ok(v) => {
            managed.succeeded();
            Ok(v)
        }
        Err(e) => {
            managed.failed(&e);
            Err(OpError::Failed(e))
        }
    }
}

#[derive(Debug)]
pub enum OpError {
    /// The guard refused or deferred the operation; nothing was executed.
    Blocked(GuardDecision),
    /// The intent could not be recorded; nothing was executed.
    AuditRefused(String),
    /// The operation ran and reported this error.
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refusals_and_stops_are_distinct_from_failures_and_children_are_linked() {
        let state = test_state().await;
        let parent = begin(
            &state,
            "s9",
            "u1",
            OperationKind::ToolCall,
            "read_file /etc/hosts".into(),
            None,
        )
        .unwrap();
        let parent_id = parent.id().to_string();

        let child_intent = OperationIntent {
            session_id: Some("s9".into()),
            task_id: None,
            parent_operation_id: Some(parent_id.clone()),
            actor: Actor {
                kind: Some(ActorKind::EmbeddedAgent),
                user_id: Some("u1".into()),
                ..Default::default()
            },
            source: Source::Chat,
            kind: OperationKind::Command,
            summary: "cat /etc/hosts".into(),
            target: Target::default(),
            cwd: None,
        };
        begin_intent(&state, &child_intent).unwrap().succeeded();
        parent.succeeded();

        begin(
            &state,
            "s9",
            "u1",
            OperationKind::Command,
            "rm -rf /".into(),
            None,
        )
        .unwrap()
        .denied("policy: destructive command token=abc123");
        begin(
            &state,
            "s9",
            "u1",
            OperationKind::Command,
            "sleep 99".into(),
            None,
        )
        .unwrap()
        .cancelled("agent stopped by user; termination not confirmed");
        begin(
            &state,
            "s9",
            "u1",
            OperationKind::McpCall,
            "fs.list".into(),
            None,
        )
        .unwrap()
        .interrupted("session closed");

        let (children, n) = audit_events::list_operations(
            &state.db,
            &audit_events::OperationFilter {
                parent_operation_id: Some(parent_id.clone()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(children[0].summary, "cat /etc/hosts");
        assert_eq!(
            children[0].parent_operation_id.as_deref(),
            Some(parent_id.as_str())
        );

        let (all, _) = audit_events::list_operations(
            &state.db,
            &audit_events::OperationFilter {
                session_id: Some("s9".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        let status_of = |needle: &str| all.iter().find(|o| o.summary.contains(needle)).unwrap();
        assert_eq!(status_of("rm -rf").status, OperationStatus::Denied);
        match &status_of("rm -rf").exit {
            Some(ExitStatus::Unknown { reason }) => {
                assert!(reason.contains("[REDACTED]"), "{}", reason)
            }
            other => panic!("{:?}", other),
        }
        assert_eq!(status_of("sleep").status, OperationStatus::Cancelled);
        assert_eq!(status_of("fs.list").status, OperationStatus::Interrupted);
        assert!(all
            .iter()
            .all(|o| o.evidence != Evidence::ExecutorConfirmed
                || o.status == OperationStatus::Succeeded));
    }

    async fn test_state() -> std::sync::Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("ngterm-audops-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
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

    #[tokio::test]
    async fn the_guard_decides_before_anything_is_recorded_or_run() {
        let state = test_state().await;
        state
            .install_guard(std::sync::Arc::new(
                crate::guard::test_support::KeywordGuard {
                    needle: "approve-me",
                },
            ))
            .unwrap();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mark = |r: &std::sync::Arc<std::sync::atomic::AtomicBool>| {
            let r = r.clone();
            async move {
                r.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<(), String>(())
            }
        };

        let deferred = run(
            &state,
            "g1",
            "u1",
            OperationKind::FileWrite,
            "write approve-me.conf".into(),
            None,
            mark(&ran),
        )
        .await;
        assert!(
            matches!(deferred, Err(OpError::Blocked(GuardDecision::AwaitApproval { ref request_id, .. })) if request_id == "req-1")
        );
        let refused = run(
            &state,
            "g1",
            "u1",
            OperationKind::FileDelete,
            "deny this".into(),
            None,
            mark(&ran),
        )
        .await;
        assert!(matches!(
            refused,
            Err(OpError::Blocked(GuardDecision::Refuse { .. }))
        ));
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        let (_, total) = audit_events::list_operations(
            &state.db,
            &audit_events::OperationFilter {
                session_id: Some("g1".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(total, 0, "blocked operations leave no operation row");

        let ok = run(
            &state,
            "g1",
            "u1",
            OperationKind::FileRead,
            "read plain".into(),
            None,
            mark(&ran),
        )
        .await;
        assert!(ok.is_ok());
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(state
            .install_guard(std::sync::Arc::new(
                crate::guard::test_support::KeywordGuard { needle: "x" }
            ))
            .is_err());
    }

    #[tokio::test]
    async fn outcome_follows_the_executor_and_dropped_handlers_are_interrupted() {
        let state = test_state().await;

        let ok: Result<(), OpError> = run(
            &state,
            "s1",
            "u1",
            OperationKind::FileWrite,
            "write /etc/app.conf".into(),
            None,
            async { Ok(()) },
        )
        .await;
        assert!(ok.is_ok());

        let failed: Result<(), OpError> = run(
            &state,
            "s1",
            "u1",
            OperationKind::FileDelete,
            "delete /etc/app.conf password=hunter2".into(),
            None,
            async { Err("permission denied".to_string()) },
        )
        .await;
        assert!(matches!(failed, Err(OpError::Failed(ref e)) if e == "permission denied"));

        let dropped = begin(
            &state,
            "s1",
            "u1",
            OperationKind::Git,
            "git diff".into(),
            None,
        )
        .unwrap();
        let dropped_id = dropped.id().to_string();
        drop(dropped);

        let (ops, total) = audit_events::list_operations(
            &state.db,
            &audit_events::OperationFilter {
                session_id: Some("s1".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(total, 3);
        let by_summary = |needle: &str| ops.iter().find(|o| o.summary.contains(needle)).unwrap();
        assert_eq!(by_summary("write").status, OperationStatus::Succeeded);
        assert_eq!(by_summary("write").evidence, Evidence::ExecutorConfirmed);
        let del = by_summary("delete");
        assert_eq!(del.status, OperationStatus::Failed);
        assert!(
            del.summary.contains("password=[REDACTED]"),
            "{}",
            del.summary
        );
        assert!(matches!(del.exit, Some(ExitStatus::Unknown { .. })));
        let git = ops.iter().find(|o| o.operation_id == dropped_id).unwrap();
        assert_eq!(git.status, OperationStatus::Interrupted);
        assert_eq!(git.actor.user_id.as_deref(), Some("u1"));
    }

    #[tokio::test]
    async fn operation_is_refused_when_the_intent_cannot_be_written() {
        let state = test_state().await;
        state
            .db
            .conn()
            .execute_batch("DROP TABLE audit_operations")
            .unwrap();
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = executed.clone();
        let result: Result<(), OpError> = run(
            &state,
            "s1",
            "u1",
            OperationKind::FileWrite,
            "write x".into(),
            None,
            async move {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err(OpError::AuditRefused(_))));
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    }
}
