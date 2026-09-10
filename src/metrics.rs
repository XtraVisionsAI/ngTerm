//! Operational visibility (ENG-05): process-wide counters for the failures
//! that must never be silent (audit writes that did not happen, recording
//! events that were dropped), disk headroom for the data directory, and
//! operations that have been "running" for suspiciously long. Exposed in
//! full to administrators via `GET /api/admin/metrics`; `/api/health` shows
//! only the resulting warning codes.
//!
//! Counters are process-lifetime and reset on restart; the audit store, not
//! this module, is the durable record.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

use crate::AppState;

static AUDIT_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);
static RECORDING_EVENTS_DROPPED: AtomicU64 = AtomicU64::new(0);
static LAST_AUDIT_WRITE_ERROR: Mutex<Option<LastError>> = Mutex::new(None);

/// Operations still `running` after this long are reported as stale. The
/// count is informational: a legitimately long upload looks the same as a
/// handler that died without its Drop guard running.
pub const STALE_OPERATION_SECS: i64 = 15 * 60;

/// Default free-space threshold below which `low_disk` is raised.
pub const DEFAULT_DISK_LOW_MB: u64 = 512;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastError {
    pub at: String,
    /// Database error text; never contains user data (SQLite messages only).
    pub error: String,
}

/// An audit record could not be written. Called by the audit store itself.
pub fn audit_write_failed(error: &str) {
    AUDIT_WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut last) = LAST_AUDIT_WRITE_ERROR.lock() {
        *last = Some(LastError {
            at: chrono::Utc::now().to_rfc3339(),
            error: error.chars().take(300).collect(),
        });
    }
}

/// A recording event was dropped because the writer queue was full.
pub fn recording_event_dropped() {
    RECORDING_EVENTS_DROPPED.fetch_add(1, Ordering::Relaxed);
}

pub fn audit_write_failures() -> u64 {
    AUDIT_WRITE_FAILURES.load(Ordering::Relaxed)
}

pub fn recording_events_dropped() -> u64 {
    RECORDING_EVENTS_DROPPED.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStatus {
    pub path: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub low_threshold_bytes: u64,
    pub low: bool,
}

/// Free-space threshold from `NGTERM_DISK_LOW_MB` (default 512).
pub fn disk_low_threshold_bytes() -> u64 {
    std::env::var("NGTERM_DISK_LOW_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_DISK_LOW_MB)
        * 1024
        * 1024
}

/// Free and total bytes of the filesystem holding `path`. `None` when the
/// platform or the path does not allow the query; callers must not treat
/// that as "enough space".
#[cfg(unix)]
// statvfs field widths differ per platform (u32 on some, u64 on others).
#[allow(clippy::unnecessary_cast)]
pub fn disk_status(path: &str) -> Option<DiskStatus> {
    use std::ffi::CString;
    let c_path = CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    let frag = st.f_frsize as u64;
    let free_bytes = st.f_bavail as u64 * frag;
    let total_bytes = st.f_blocks as u64 * frag;
    let low_threshold_bytes = disk_low_threshold_bytes();
    Some(DiskStatus {
        path: path.to_string(),
        free_bytes,
        total_bytes,
        low_threshold_bytes,
        low: free_bytes < low_threshold_bytes,
    })
}

#[cfg(not(unix))]
pub fn disk_status(_path: &str) -> Option<DiskStatus> {
    None
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationCounts {
    /// Registered as running right now.
    pub running: u32,
    /// Running for longer than `staleAfterSecs`; end never observed so far.
    pub stale: u32,
    pub stale_after_secs: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMetrics {
    pub enabled: bool,
    /// Events dropped across all recorders since process start. Each drop is
    /// also a `gap` marker inside the affected recording.
    pub events_dropped: u64,
    pub queue_capacity: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub taken_at: String,
    pub audit_write_failures: u64,
    pub last_audit_write_error: Option<LastError>,
    pub recording: RecordingMetrics,
    pub disk: Option<DiskStatus>,
    pub operations: OperationCounts,
    pub active_sessions: usize,
    pub active_agents: usize,
    /// Machine-readable conditions an operator should act on.
    pub warnings: Vec<&'static str>,
}

pub const WARN_AUDIT_WRITE_FAILURES: &str = "audit_write_failures";
pub const WARN_RECORDING_DROPS: &str = "recording_drops";
pub const WARN_LOW_DISK: &str = "low_disk";
pub const WARN_DISK_UNKNOWN: &str = "disk_unknown";
pub const WARN_STALE_OPERATIONS: &str = "stale_operations";
pub const WARN_UNCLEAN_PREVIOUS_SHUTDOWN: &str = "unclean_previous_shutdown";
pub const WARN_RECORDING_DISABLED: &str = "recording_disabled";

pub async fn snapshot(state: &AppState) -> Snapshot {
    let stale_before =
        (chrono::Utc::now() - chrono::Duration::seconds(STALE_OPERATION_SECS)).to_rfc3339();
    let (running, stale) = crate::audit_events::count_running_operations(&state.db, &stale_before)
        .unwrap_or_else(|e| {
            tracing::error!("Could not count running operations: {}", e);
            (0, 0)
        });
    let disk = disk_status(&state.config.data_dir);
    let recording = RecordingMetrics {
        enabled: state.recordings.is_some(),
        events_dropped: recording_events_dropped(),
        queue_capacity: state.config.recording.queue_capacity,
    };
    let audit_write_failures = audit_write_failures();

    let mut warnings = Vec::new();
    if audit_write_failures > 0 {
        warnings.push(WARN_AUDIT_WRITE_FAILURES);
    }
    if recording.events_dropped > 0 {
        warnings.push(WARN_RECORDING_DROPS);
    }
    match &disk {
        Some(d) if d.low => warnings.push(WARN_LOW_DISK),
        Some(_) => {}
        None => warnings.push(WARN_DISK_UNKNOWN),
    }
    if stale > 0 {
        warnings.push(WARN_STALE_OPERATIONS);
    }
    if state.startup.previous_shutdown_clean == Some(false) {
        warnings.push(WARN_UNCLEAN_PREVIOUS_SHUTDOWN);
    }
    if !recording.enabled {
        warnings.push(WARN_RECORDING_DISABLED);
    }

    Snapshot {
        taken_at: chrono::Utc::now().to_rfc3339(),
        audit_write_failures,
        last_audit_write_error: LAST_AUDIT_WRITE_ERROR.lock().ok().and_then(|l| l.clone()),
        recording,
        disk,
        operations: OperationCounts {
            running,
            stale,
            stale_after_secs: STALE_OPERATION_SECS,
        },
        active_sessions: state.sessions.list_all_session_count(),
        active_agents: state.agents.active_count().await,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_status_reports_the_data_dir_filesystem() {
        let dir = std::env::temp_dir();
        let d = disk_status(dir.to_str().unwrap()).expect("statvfs on temp dir");
        assert!(d.total_bytes > 0);
        assert!(d.free_bytes <= d.total_bytes);
        assert_eq!(d.low, d.free_bytes < d.low_threshold_bytes);
        assert!(disk_status("/definitely/not/here/ngterm").is_none());
    }

    async fn test_state() -> std::sync::Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("ngterm-metrics-{}", uuid::Uuid::new_v4()));
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

    #[tokio::test]
    async fn snapshot_flags_stale_operations_and_counts_failed_audit_writes() {
        use crate::audit_events::{self, Actor, OperationIntent, OperationKind, Source, Target};
        let state = test_state().await;
        let clean = snapshot(&state).await;
        assert_eq!(clean.operations.running, 0);
        assert!(!clean.warnings.contains(&WARN_STALE_OPERATIONS));
        assert!(clean.disk.is_some());

        let intent = OperationIntent {
            session_id: Some("s1".into()),
            task_id: None,
            parent_operation_id: None,
            actor: Actor {
                user_id: Some("u1".into()),
                ..Default::default()
            },
            source: Source::Api,
            kind: OperationKind::Upload,
            summary: "upload big.iso".into(),
            target: Target::default(),
            cwd: None,
        };
        let id = audit_events::operation_intended(&state.db, &intent).unwrap();
        audit_events::operation_started(&state.db, &id).unwrap();
        let live = snapshot(&state).await;
        assert_eq!(live.operations.running, 1);
        assert_eq!(live.operations.stale, 0);

        // Age the operation past the threshold (operations are not
        // append-only; only their outcome is guarded by the API).
        state
            .db
            .conn()
            .execute(
                "UPDATE audit_operations SET started_at = '2000-01-01T00:00:00+00:00' WHERE operation_id = ?1",
                [&id],
            )
            .unwrap();
        let stale = snapshot(&state).await;
        assert_eq!(stale.operations.stale, 1);
        assert!(stale.warnings.contains(&WARN_STALE_OPERATIONS));

        // A write the store cannot perform is counted, not swallowed.
        let before = audit_write_failures();
        state
            .db
            .conn()
            .execute_batch("DROP TABLE audit_events")
            .unwrap();
        let failed = audit_events::append_event(
            &state.db,
            audit_events::NewEvent {
                stream_id: "config",
                occurred_at: None,
                session_id: None,
                operation_id: None,
                event_type: "config.change",
                payload: serde_json::json!({}),
                integrity: audit_events::Integrity::Complete,
            },
        );
        assert!(failed.is_err());
        assert!(audit_write_failures() > before);
        assert!(snapshot(&state)
            .await
            .warnings
            .contains(&WARN_AUDIT_WRITE_FAILURES));
    }

    #[test]
    fn failure_counter_keeps_the_last_error() {
        let before = audit_write_failures();
        audit_write_failed("database is locked");
        assert_eq!(audit_write_failures(), before + 1);
        let last = LAST_AUDIT_WRITE_ERROR.lock().unwrap().clone().unwrap();
        assert_eq!(last.error, "database is locked");
    }
}
