//! Configuration-change audit (AUD-09): administrative and per-user changes
//! to users, servers, SSH keys, AI tools and tool configurations are
//! recorded as `config_change` operations with a `config.change` event that
//! carries redacted before/after snapshots and the changed field names.
//!
//! Credentials never reach the audit store: passwords are omitted entirely,
//! SSH keys and tool environment values appear only as metadata (names,
//! fingerprints, key names). The intent is written before the change is
//! applied (AUD-08); if it cannot be written the change is refused.

use std::future::Future;

use serde::Serialize;
use serde_json::{json, Value};

use crate::ai_tool_registry::AiTool;
use crate::audit_events::{
    self, Actor, ActorKind, Integrity, NewEvent, OperationIntent, OperationKind, Source, Target,
};
use crate::audit_ops::{self, OpError};
use crate::extractors::Caller;
use crate::key_manager::KeyInfo;
use crate::server_registry::Server;
use crate::server_tool_config::ServerToolConfig;
use crate::user_tool_config::UserToolConfig;
use crate::AppState;

/// Stream that holds every configuration-change event.
pub const CONFIG_STREAM: &str = "config";
/// Event type appended for each recorded change (successful or not).
pub const CHANGE_EVENT: &str = "config.change";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    User,
    Server,
    SshKey,
    AiTool,
    UserToolConfig,
    ServerToolConfig,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Update,
    Delete,
    PasswordChange,
    PasswordReset,
}

/// What is about to change. `before` is the redacted snapshot taken by the
/// caller before applying the change (absent for creations).
pub struct Change {
    pub object: ObjectKind,
    pub object_id: Option<String>,
    pub action: Action,
    pub summary: String,
    pub target: Target,
    pub before: Option<Value>,
}

impl Change {
    pub fn new(object: ObjectKind, action: Action, summary: impl Into<String>) -> Self {
        Change {
            object,
            object_id: None,
            action,
            summary: summary.into(),
            target: Target::default(),
            before: None,
        }
    }

    pub fn object_id(mut self, id: impl Into<String>) -> Self {
        self.object_id = Some(id.into());
        self
    }

    pub fn target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }

    pub fn before(mut self, snapshot: Option<Value>) -> Self {
        self.before = snapshot;
        self
    }
}

/// Result of applying a change: the value to return to the caller plus the
/// redacted snapshot afterwards (absent for deletions) and, for creations,
/// the id the object received.
pub struct Applied<T> {
    pub value: T,
    pub object_id: Option<String>,
    pub after: Option<Value>,
}

impl<T> Applied<T> {
    pub fn new(value: T) -> Self {
        Applied {
            value,
            object_id: None,
            after: None,
        }
    }

    pub fn object_id(mut self, id: impl Into<String>) -> Self {
        self.object_id = Some(id.into());
        self
    }

    pub fn after(mut self, snapshot: Option<Value>) -> Self {
        self.after = snapshot;
        self
    }
}

fn username_of(state: &AppState, caller: &Caller) -> Option<String> {
    if caller.is_admin {
        return Some("admin".to_string());
    }
    state
        .db
        .conn()
        .query_row(
            "SELECT username FROM users WHERE id = ?1",
            rusqlite::params![caller.user_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
}

/// Top-level keys whose value differs between the two snapshots.
pub fn changed_fields(before: Option<&Value>, after: Option<&Value>) -> Vec<String> {
    let empty = serde_json::Map::new();
    let b = before.and_then(Value::as_object).unwrap_or(&empty);
    let a = after.and_then(Value::as_object).unwrap_or(&empty);
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| b.get(*k) != a.get(*k))
        .cloned()
        .collect()
}

/// Record `change` as an operation performed by `caller`, apply it via
/// `op`, then store the truthful outcome together with the snapshots.
/// Refuses to apply anything when the intent cannot be written.
pub async fn run<T, F>(
    state: &AppState,
    caller: &Caller,
    change: Change,
    op: F,
) -> Result<T, OpError>
where
    F: Future<Output = Result<Applied<T>, String>>,
{
    let actor = Actor {
        kind: Some(ActorKind::Human),
        user_id: Some(caller.user_id.clone()),
        username: username_of(state, caller),
        ..Default::default()
    };
    let intent = OperationIntent {
        session_id: None,
        task_id: None,
        parent_operation_id: None,
        actor,
        source: Source::Api,
        kind: OperationKind::ConfigChange,
        summary: change.summary.clone(),
        target: change.target.clone(),
        cwd: None,
    };
    let managed = audit_ops::begin_intent(state, &intent).map_err(OpError::AuditRefused)?;
    let operation_id = managed.id().to_string();

    let result = op.await;
    let (outcome, object_id, after, error) = match &result {
        Ok(applied) => (
            "succeeded",
            applied.object_id.clone().or(change.object_id.clone()),
            applied.after.clone(),
            None,
        ),
        Err(e) => (
            "failed",
            change.object_id.clone(),
            None,
            Some(audit_events::redact(e)),
        ),
    };
    let payload = json!({
        "object": change.object,
        "objectId": object_id,
        "action": change.action,
        "outcome": outcome,
        "error": error,
        "before": change.before,
        "after": after,
        "changedFields": if result.is_ok() {
            changed_fields(change.before.as_ref(), after.as_ref())
        } else {
            Vec::new()
        },
    });
    if let Err(e) = audit_events::append_event(
        &state.db,
        NewEvent {
            stream_id: CONFIG_STREAM,
            occurred_at: None,
            session_id: None,
            operation_id: Some(&operation_id),
            event_type: CHANGE_EVENT,
            payload,
            integrity: Integrity::Complete,
        },
    ) {
        tracing::error!(
            "Configuration change {} applied but its snapshot could not be recorded: {}",
            operation_id,
            e
        );
    }
    match result {
        Ok(applied) => {
            managed.succeeded();
            Ok(applied.value)
        }
        Err(e) => {
            managed.failed(&e);
            Err(OpError::Failed(e))
        }
    }
}

// --- Snapshots ----------------------------------------------------------------
//
// Every snapshot is built explicitly so that no secret-bearing field can
// slip in by serialising a whole struct.

pub fn user_snapshot(user: &Value) -> Value {
    json!({
        "id": user.get("id"),
        "username": user.get("username"),
        "role": user.get("role"),
    })
}

pub fn server_snapshot(s: &Server) -> Value {
    json!({
        "id": s.id,
        "groupName": s.group_name,
        "alias": s.alias,
        "host": s.host,
        "port": s.port,
        "username": s.username,
        "keyId": s.key_id,
        "tags": s.tags,
        "aiToolId": s.ai_tool_id,
        "hostKeyFingerprint": s.host_key_fingerprint,
        "idleTimeoutSecs": s.idle_timeout_secs,
    })
}

pub fn server_target(s: &Server) -> Target {
    Target {
        server_id: Some(s.id.clone()),
        server_alias: Some(s.alias.clone()),
        server_host: Some(s.host.clone()),
        remote_user: Some(s.username.clone()),
    }
}

/// SSH key metadata only; the key material is never part of any record.
pub fn key_snapshot(k: &KeyInfo) -> Value {
    json!({
        "id": k.id,
        "name": k.name,
        "fingerprint": k.fingerprint,
        "keyType": k.key_type,
    })
}

/// Tool definition with the default value of every parameter declared
/// `secret` masked. Key-based redaction is applied again on append.
pub fn tool_snapshot(t: &AiTool) -> Value {
    let mut options = t.options.clone();
    if let Some(params) = options.get_mut("params").and_then(Value::as_array_mut) {
        for p in params {
            let secret = p.get("secret").and_then(Value::as_bool).unwrap_or(false);
            if secret {
                if let Some(obj) = p.as_object_mut() {
                    if obj.contains_key("default") {
                        obj.insert("default".into(), Value::String("[REDACTED]".into()));
                    }
                }
            }
        }
    }
    json!({
        "id": t.id,
        "name": t.name,
        "displayName": t.display_name,
        "type": t.tool_type,
        "options": options,
    })
}

/// User-level tool configuration: environment values are represented by
/// the names that were submitted, never their contents.
pub fn user_tool_config_snapshot(c: &UserToolConfig, env_keys: Option<Vec<String>>) -> Value {
    json!({
        "id": c.id,
        "toolId": c.tool_id,
        "configOverride": c.config_override,
        "disabledKeys": c.disabled_keys,
        "hasEnvValues": c.has_env_values,
        "envKeysSubmitted": env_keys,
    })
}

pub fn server_tool_config_snapshot(c: &ServerToolConfig, env_keys: Option<Vec<String>>) -> Value {
    json!({
        "id": c.id,
        "toolId": c.tool_id,
        "serverId": c.server_id,
        "configOverride": c.config_override,
        "disabledKeys": c.disabled_keys,
        "hasEnvOverrides": c.has_env_overrides,
        "envKeysSubmitted": env_keys,
    })
}

/// Sorted key names of a submitted environment map (values discarded).
pub fn env_key_names<V>(env: Option<&std::collections::HashMap<String, V>>) -> Option<Vec<String>> {
    env.map(|m| {
        let mut keys: Vec<String> = m.keys().cloned().collect();
        keys.sort();
        keys
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_events::{OperationFilter, OperationStatus};

    async fn test_state() -> std::sync::Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("ngterm-audcfg-{}", uuid::Uuid::new_v4()));
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

    fn admin() -> Caller {
        Caller {
            user_id: "admin".into(),
            is_admin: true,
        }
    }

    #[tokio::test]
    async fn change_records_operation_and_redacted_snapshots() {
        let state = test_state().await;
        let before = json!({"alias": "db", "port": 22, "apiToken": "abc"});
        let change = Change::new(ObjectKind::Server, Action::Update, "update server db")
            .object_id("srv1")
            .before(Some(before));
        let out: Result<u32, OpError> = run(&state, &admin(), change, async {
            Ok(Applied::new(7).after(Some(
                json!({"alias": "db", "port": 2222, "apiToken": "abc"}),
            )))
        })
        .await;
        assert_eq!(out.unwrap(), 7);

        let (ops, total) = audit_events::list_operations(
            &state.db,
            &OperationFilter {
                kind: Some(OperationKind::ConfigChange),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(total, 1);
        let op = &ops[0];
        assert_eq!(op.status, OperationStatus::Succeeded);
        assert_eq!(op.actor.user_id.as_deref(), Some("admin"));
        assert_eq!(op.actor.username.as_deref(), Some("admin"));
        assert!(op.session_id.is_none());

        let events = audit_events::events_for_operation(&state.db, &op.operation_id).unwrap();
        assert_eq!(events.len(), 1);
        let p = &events[0].payload;
        assert_eq!(events[0].stream_id, CONFIG_STREAM);
        assert_eq!(p["object"], "server");
        assert_eq!(p["action"], "update");
        assert_eq!(p["objectId"], "srv1");
        assert_eq!(p["outcome"], "succeeded");
        assert_eq!(p["changedFields"], json!(["port"]));
        assert_eq!(p["before"]["port"], 22);
        assert_eq!(p["after"]["port"], 2222);
        // Key-based redaction applies to snapshots as to any payload.
        assert_eq!(p["before"]["apiToken"], "[REDACTED]");
        assert_eq!(p["after"]["apiToken"], "[REDACTED]");
    }

    #[tokio::test]
    async fn failed_change_is_recorded_as_failed_with_redacted_reason() {
        let state = test_state().await;
        let change = Change::new(ObjectKind::User, Action::Create, "create user bob");
        let out: Result<(), OpError> = run(&state, &admin(), change, async {
            Err("db locked while inserting password=hunter2".to_string())
        })
        .await;
        assert!(matches!(out, Err(OpError::Failed(_))));
        let (ops, _) =
            audit_events::list_operations(&state.db, &OperationFilter::default(), 10, 0).unwrap();
        assert_eq!(ops[0].status, OperationStatus::Failed);
        let events = audit_events::events_for_operation(&state.db, &ops[0].operation_id).unwrap();
        assert_eq!(events[0].payload["outcome"], "failed");
        assert_eq!(
            events[0].payload["error"],
            "db locked while inserting password=[REDACTED]"
        );
        assert!(events[0].payload["changedFields"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn change_is_refused_when_intent_cannot_be_written() {
        let state = test_state().await;
        state
            .db
            .conn()
            .execute_batch("DROP TABLE audit_operations")
            .unwrap();
        let applied = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = applied.clone();
        let out: Result<(), OpError> = run(
            &state,
            &admin(),
            Change::new(ObjectKind::AiTool, Action::Delete, "delete tool t1"),
            async move {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Applied::new(()))
            },
        )
        .await;
        assert!(matches!(out, Err(OpError::AuditRefused(_))));
        assert!(!applied.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn snapshots_carry_metadata_but_no_secrets() {
        let tool = AiTool {
            id: "t1".into(),
            name: "x".into(),
            display_name: "X".into(),
            tool_type: "external".into(),
            options: json!({
                "params": [
                    {"key": "API_KEY", "secret": true, "default": "sk-live-1234567890"},
                    {"key": "MODEL", "secret": false, "default": "gpt"}
                ]
            }),
            created_at: "now".into(),
        };
        let snap = tool_snapshot(&tool);
        assert_eq!(snap["options"]["params"][0]["default"], "[REDACTED]");
        assert_eq!(snap["options"]["params"][1]["default"], "gpt");
        // The `secret` flag itself must survive key-based redaction on append.
        assert_eq!(
            audit_events::redact_json(snap.clone())["options"]["params"][0]["secret"],
            true
        );

        let key = KeyInfo {
            id: "k1".into(),
            name: "deploy".into(),
            fingerprint: "SHA256:abc".into(),
            key_type: "ed25519".into(),
            created_at: "now".into(),
        };
        let snap = key_snapshot(&key);
        assert_eq!(snap["fingerprint"], "SHA256:abc");
        assert!(snap.get("privateKey").is_none());

        let mut env = std::collections::HashMap::new();
        env.insert("TOKEN".to_string(), "secret-value".to_string());
        env.insert("BASE_URL".to_string(), "https://x".to_string());
        let keys = env_key_names(Some(&env)).unwrap();
        assert_eq!(keys, vec!["BASE_URL", "TOKEN"]);

        assert_eq!(
            changed_fields(
                Some(&json!({"a": 1, "b": 2})),
                Some(&json!({"a": 1, "b": 3, "c": 4}))
            ),
            vec!["b", "c"]
        );
    }
}
