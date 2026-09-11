//! Neutral pre-execution check point. Before the platform opens a session
//! on a user's behalf or runs a managed operation through its own channel,
//! it asks the installed [`ExecutionGuard`] whether to proceed. The guard is
//! a server-side component installed by the distribution at start-up; the
//! open-source build installs none and everything proceeds as before.
//!
//! The contract is deliberately narrow so the decision can only rest on
//! facts the server holds: who is asking (authenticated user id, admin
//! flag), what exactly is about to happen (the same intent that is written
//! to the audit store), and where. Nothing a client, a model or a tool
//! result says can enter the decision except through that intent.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;

use crate::audit_events::OperationIntent;

/// What is about to happen.
#[derive(Debug, Clone)]
pub enum GuardAction<'a> {
    /// A human wants an interactive session on `server_id` as `remote_user`.
    SessionAdmission {
        server_id: &'a str,
        remote_user: &'a str,
    },
    /// A managed operation with exactly this intent.
    Operation { intent: &'a OperationIntent },
}

#[derive(Debug, Clone)]
pub struct GuardRequest<'a> {
    /// Authenticated platform user asking for the action.
    pub user_id: &'a str,
    pub is_admin: bool,
    pub action: GuardAction<'a>,
}

/// Outcome of the check. Serialised into the HTTP answer when it is not
/// `Proceed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GuardDecision {
    Proceed,
    /// Do not run; the reason is shown to the requester.
    Refuse {
        reason: String,
    },
    /// Do not run now; someone else has to approve request `request_id`
    /// first. The requester retries the same action once it is approved.
    AwaitApproval {
        request_id: String,
        message: String,
    },
}

pub type GuardFuture<'a> = Pin<Box<dyn Future<Output = GuardDecision> + Send + 'a>>;

pub trait ExecutionGuard: Send + Sync {
    fn check<'a>(&'a self, request: &'a GuardRequest<'a>) -> GuardFuture<'a>;
}

/// Run the installed guard, if any. Without a guard every action proceeds.
pub async fn check(
    guard: Option<&Arc<dyn ExecutionGuard>>,
    request: GuardRequest<'_>,
) -> GuardDecision {
    match guard {
        Some(g) => g.check(&request).await,
        None => GuardDecision::Proceed,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Guard for tests: requires approval for every operation whose summary
    /// contains `needle`, refuses everything containing `deny`.
    pub struct KeywordGuard {
        pub needle: &'static str,
    }

    impl ExecutionGuard for KeywordGuard {
        fn check<'a>(&'a self, request: &'a GuardRequest<'a>) -> GuardFuture<'a> {
            Box::pin(async move {
                match &request.action {
                    GuardAction::Operation { intent } if intent.summary.contains("deny") => {
                        GuardDecision::Refuse {
                            reason: "denied by test policy".into(),
                        }
                    }
                    GuardAction::Operation { intent } if intent.summary.contains(self.needle) => {
                        GuardDecision::AwaitApproval {
                            request_id: "req-1".into(),
                            message: "needs a second person".into(),
                        }
                    }
                    GuardAction::SessionAdmission { server_id, .. } if *server_id == "locked" => {
                        GuardDecision::AwaitApproval {
                            request_id: "req-s".into(),
                            message: "session admission approval".into(),
                        }
                    }
                    _ => GuardDecision::Proceed,
                }
            })
        }
    }
}
