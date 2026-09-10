//! Audit store maintenance (AUD-10): append-only protection at the database
//! level, retention purges that are the only sanctioned way to remove audit
//! history, integrity checks, and file-level backup.
//!
//! The guarantees are deliberately modest and stated as such: the triggers
//! stop the application and ad-hoc SQL from editing or deleting audit rows
//! outside a maintenance window; they cannot protect against someone with
//! full control of the host, who can drop the triggers or edit the file.
//! Chunk hashes detect corruption and truncation, not deliberate rewriting
//! by such an actor.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection, Transaction};
use serde::Serialize;
use serde_json::json;

use crate::audit_events::{self, Integrity, NewEvent};
use crate::db::Database;
use crate::recording::{ChunkProblem, RecordingStore};

/// Stream for retention and maintenance records (never purged itself).
pub const MAINTENANCE_STREAM: &str = "system";
pub const RETENTION_EVENT: &str = "system.retention";

/// Plain statement of what the local checks can and cannot show.
pub const LIMITATION_NOTE: &str = "Append-only triggers and chunk hashes detect application bugs, accidental edits, truncation and bit rot. They do not protect against an actor with full control of the host, who can drop the triggers or rewrite files and hashes together.";

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Tables whose rows may only be deleted inside a maintenance window.
const GUARDED_DELETE: &[&str] = &[
    "audit_events",
    "audit_sessions",
    "audit_operations",
    "audit_recordings",
    "audit_recording_chunks",
    "audit_logs",
];

/// Migration v4: the maintenance-window table and the guard triggers.
/// `audit_events` additionally rejects every UPDATE; the other tables keep
/// their legitimate state transitions (session ended, operation finished).
pub fn migration_v4_append_only(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut sql = String::from(
        "
        CREATE TABLE IF NOT EXISTS audit_maintenance_window (
            id        INTEGER PRIMARY KEY CHECK (id = 1),
            opened_at TEXT NOT NULL
        );
        CREATE TRIGGER IF NOT EXISTS audit_events_no_update BEFORE UPDATE ON audit_events
        BEGIN
            SELECT RAISE(ABORT, 'audit_events is append-only');
        END;
        ",
    );
    for table in GUARDED_DELETE {
        sql.push_str(&format!(
            "CREATE TRIGGER IF NOT EXISTS {t}_guarded_delete BEFORE DELETE ON {t}
             WHEN NOT EXISTS (SELECT 1 FROM audit_maintenance_window)
             BEGIN
                 SELECT RAISE(ABORT, 'audit history can only be removed by the retention policy');
             END;\n",
            t = table
        ));
    }
    conn.execute_batch(&sql)
}

/// Run `f` inside a transaction during which deletes from the audit tables
/// are permitted. The window row lives only inside the transaction, so it
/// is never visible after commit or rollback.
pub fn with_maintenance_window<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO audit_maintenance_window (id, opened_at) VALUES (1, ?1)",
        params![chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    let out = f(&tx)?;
    tx.execute("DELETE FROM audit_maintenance_window WHERE id = 1", [])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Whether the guard triggers are present (someone with file access could
/// have dropped them; the integrity report says so instead of assuming).
pub fn guards_present(db: &Database) -> bool {
    let expected = GUARDED_DELETE.len() + 1;
    db.conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND (name LIKE '%_guarded_delete' OR name = 'audit_events_no_update')",
            [],
            |r| r.get::<_, usize>(0),
        )
        .map(|n| n >= expected)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Administrator policy for how long audit rows are kept. Recordings have
/// their own policy in [`crate::recording::RecordingConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub days: u32,
}

impl RetentionPolicy {
    /// `NGTERM_AUDIT_RETENTION_DAYS`; unset, zero or unparsable = keep forever.
    pub fn from_env() -> Option<Self> {
        std::env::var("NGTERM_AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|d| *d > 0)
            .map(|days| RetentionPolicy { days })
    }
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    pub cutoff: String,
    pub events_removed: usize,
    pub operations_removed: usize,
    pub sessions_removed: usize,
    pub legacy_logs_removed: usize,
}

/// Remove finished audit rows older than the policy. Only rows that have
/// reached a final state are eligible; live sessions and running
/// operations are never touched. System events are exempt so the record of
/// starts, shutdowns and purges is never lost. The purge itself is recorded.
pub fn purge_older_than(db: &Database, policy: RetentionPolicy) -> Result<PurgeReport, String> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(policy.days as i64)).to_rfc3339();
    let report = {
        let mut conn = db.conn();
        with_maintenance_window(&mut conn, |tx| {
            let events_removed = tx
                .execute(
                    "DELETE FROM audit_events WHERE recorded_at < ?1 AND stream_id != ?2",
                    params![cutoff, MAINTENANCE_STREAM],
                )
                .map_err(|e| e.to_string())?;
            let operations_removed = tx
                .execute(
                    "DELETE FROM audit_operations WHERE finished_at IS NOT NULL AND finished_at < ?1",
                    params![cutoff],
                )
                .map_err(|e| e.to_string())?;
            let sessions_removed = tx
                .execute(
                    "DELETE FROM audit_sessions WHERE disconnected_at IS NOT NULL AND disconnected_at < ?1",
                    params![cutoff],
                )
                .map_err(|e| e.to_string())?;
            let legacy_logs_removed = tx
                .execute(
                    "DELETE FROM audit_logs WHERE disconnected_at IS NOT NULL AND disconnected_at < ?1",
                    params![cutoff],
                )
                .map_err(|e| e.to_string())?;
            Ok(PurgeReport {
                cutoff: cutoff.clone(),
                events_removed,
                operations_removed,
                sessions_removed,
                legacy_logs_removed,
            })
        })?
    };
    let removed = report.events_removed
        + report.operations_removed
        + report.sessions_removed
        + report.legacy_logs_removed;
    if removed > 0 {
        audit_events::append_event(
            db,
            NewEvent {
                stream_id: MAINTENANCE_STREAM,
                occurred_at: None,
                session_id: None,
                operation_id: None,
                event_type: RETENTION_EVENT,
                payload: json!({
                    "policyDays": policy.days,
                    "cutoff": report.cutoff,
                    "eventsRemoved": report.events_removed,
                    "operationsRemoved": report.operations_removed,
                    "sessionsRemoved": report.sessions_removed,
                    "legacyLogsRemoved": report.legacy_logs_removed,
                }),
                integrity: Integrity::Complete,
            },
        )?;
    }
    Ok(report)
}

/// Apply the policy periodically. Nothing is spawned without a policy.
pub fn spawn_retention_task(db: Database, policy: Option<RetentionPolicy>, every: Duration) {
    let Some(policy) = policy else {
        return;
    };
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(every);
        loop {
            interval.tick().await;
            let db = db.clone();
            match tokio::task::spawn_blocking(move || purge_older_than(&db, policy)).await {
                Ok(Ok(r))
                    if r.events_removed
                        + r.operations_removed
                        + r.sessions_removed
                        + r.legacy_logs_removed
                        > 0 =>
                {
                    tracing::info!(
                        "Audit retention removed {} event(s), {} operation(s), {} session(s)",
                        r.events_removed,
                        r.operations_removed,
                        r.sessions_removed
                    )
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::error!("Audit retention failed: {}", e),
                Err(e) => tracing::error!("Audit retention task panicked: {}", e),
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Integrity
// ---------------------------------------------------------------------------

/// A hole in a stream's sequence between two existing rows. A missing
/// prefix (oldest rows purged by retention) is not a gap.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StreamGap {
    pub stream_id: String,
    pub after_seq: i64,
    pub next_seq: i64,
}

/// Scan every stream for sequence holes.
pub fn check_stream_continuity(db: &Database) -> Result<(usize, Vec<StreamGap>), String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT stream_id, seq FROM audit_events ORDER BY stream_id, seq")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut gaps = Vec::new();
    let mut streams = 0;
    let mut last: Option<(String, i64)> = None;
    for row in rows.filter_map(Result::ok) {
        match &last {
            Some((stream, seq)) if *stream == row.0 => {
                if row.1 != seq + 1 {
                    gaps.push(StreamGap {
                        stream_id: row.0.clone(),
                        after_seq: *seq,
                        next_seq: row.1,
                    });
                }
            }
            _ => streams += 1,
        }
        last = Some(row);
    }
    Ok((streams, gaps))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingProblems {
    pub recording_id: String,
    pub session_id: String,
    pub problems: Vec<ChunkProblem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReport {
    pub checked_at: String,
    pub append_only_guards_present: bool,
    pub streams_checked: usize,
    pub stream_gaps: Vec<StreamGap>,
    pub recordings_checked: usize,
    pub recordings_with_problems: Vec<RecordingProblems>,
    pub limitation: &'static str,
}

/// Verify the newest `recording_limit` finished recordings chunk by chunk
/// and the event streams for holes. Bounded so it can run on request.
pub fn integrity_report(
    db: &Database,
    recordings: Option<&Arc<RecordingStore>>,
    recording_limit: u32,
) -> Result<IntegrityReport, String> {
    let (streams_checked, stream_gaps) = check_stream_continuity(db)?;
    let mut recordings_checked = 0;
    let mut recordings_with_problems = Vec::new();
    if let Some(store) = recordings {
        for meta in store.list_recent(recording_limit)? {
            recordings_checked += 1;
            let problems = store.verify(&meta.recording_id)?;
            if !problems.is_empty() {
                recordings_with_problems.push(RecordingProblems {
                    recording_id: meta.recording_id,
                    session_id: meta.session_id,
                    problems,
                });
            }
        }
    }
    Ok(IntegrityReport {
        checked_at: chrono::Utc::now().to_rfc3339(),
        append_only_guards_present: guards_present(db),
        streams_checked,
        stream_gaps,
        recordings_checked,
        recordings_with_problems,
        limitation: LIMITATION_NOTE,
    })
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    pub created_at: String,
    pub destination: PathBuf,
    pub schema_version: u32,
    pub database_bytes: u64,
    pub recordings_copied: usize,
    pub chunk_files_copied: usize,
    pub recording_bytes: u64,
    pub partial_files_skipped: usize,
}

/// Consistent copy of the database (SQLite `VACUUM INTO`, which snapshots
/// a single transaction) plus every finished recording chunk, into an empty
/// directory with a manifest. Restore = stop the server, put `onemux.db`
/// and `recordings/` back into the data directory, start; the startup
/// recovery and the integrity report then confirm index and files agree.
pub fn backup(db: &Database, recording_root: &Path, dest: &Path) -> Result<BackupReport, String> {
    if dest.exists() {
        let mut entries = std::fs::read_dir(dest).map_err(|e| format!("{:?}: {}", dest, e))?;
        if entries.next().is_some() {
            return Err(format!(
                "backup destination {:?} exists and is not empty; refusing to overwrite",
                dest
            ));
        }
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {:?}: {}", dest, e))?;
    crate::recording::restrict_permissions(dest, 0o700);

    let db_path = dest.join("onemux.db");
    let target = db_path
        .to_str()
        .ok_or_else(|| "backup path is not valid UTF-8".to_string())?;
    db.conn()
        .execute("VACUUM INTO ?1", params![target])
        .map_err(|e| format!("database snapshot: {}", e))?;
    crate::recording::restrict_permissions(&db_path, 0o600);
    let database_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    let mut report = BackupReport {
        created_at: chrono::Utc::now().to_rfc3339(),
        destination: dest.to_path_buf(),
        schema_version: db.schema_version(),
        database_bytes,
        recordings_copied: 0,
        chunk_files_copied: 0,
        recording_bytes: 0,
        partial_files_skipped: 0,
    };
    if recording_root.is_dir() {
        let out_root = dest.join("recordings");
        std::fs::create_dir_all(&out_root).map_err(|e| e.to_string())?;
        crate::recording::restrict_permissions(&out_root, 0o700);
        for entry in std::fs::read_dir(recording_root)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if !entry.path().is_dir() {
                continue;
            }
            let out_dir = out_root.join(entry.file_name());
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            crate::recording::restrict_permissions(&out_dir, 0o700);
            report.recordings_copied += 1;
            for f in std::fs::read_dir(entry.path())
                .map_err(|e| e.to_string())?
                .flatten()
            {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) == Some("part") {
                    report.partial_files_skipped += 1;
                    continue;
                }
                let out = out_dir.join(f.file_name());
                let n =
                    std::fs::copy(&path, &out).map_err(|e| format!("copy {:?}: {}", path, e))?;
                crate::recording::restrict_permissions(&out, 0o600);
                report.chunk_files_copied += 1;
                report.recording_bytes += n;
            }
        }
    }
    let manifest = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let manifest_path = dest.join("MANIFEST.json");
    std::fs::write(&manifest_path, manifest).map_err(|e| e.to_string())?;
    crate::recording::restrict_permissions(&manifest_path, 0o600);
    Ok(report)
}

/// `--backup-to` for the binaries: back up into `dest` and return the
/// manifest as pretty JSON for stdout.
pub fn backup_cli(db: &Database, data_dir: &str, dest: &str) -> Result<String, String> {
    let root = crate::recording::RecordingConfig::from_env(data_dir).root;
    let report = backup(db, &root, Path::new(dest))?;
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_events::{
        Actor, AuditSession, OperationIntent, OperationKind, Source, Target,
    };

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ngterm-audmt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed(db: &Database, sid: &str, when: &str) -> String {
        audit_events::session_started(
            db,
            &AuditSession {
                session_id: sid.into(),
                actor: Actor::default(),
                target: Target::default(),
                source: Source::Terminal,
                parent_session_id: None,
                connected_at: when.into(),
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
                summary: "ls".into(),
                target: Target::default(),
                cwd: None,
            },
        )
        .unwrap();
        audit_events::append_event(
            db,
            NewEvent {
                stream_id: sid,
                occurred_at: None,
                session_id: Some(sid),
                operation_id: Some(&op),
                event_type: "test",
                payload: json!({}),
                integrity: Integrity::Complete,
            },
        )
        .unwrap();
        op
    }

    /// Age every row of a session so it falls before a retention cutoff.
    fn age(db: &Database, sid: &str, to: &str) {
        let conn = db.conn();
        conn.execute(
            "UPDATE audit_sessions SET disconnected_at = ?1 WHERE session_id = ?2",
            params![to, sid],
        )
        .unwrap();
        conn.execute(
            "UPDATE audit_operations SET status = 'succeeded', finished_at = ?1 WHERE session_id = ?2",
            params![to, sid],
        )
        .unwrap();
        // audit_events rejects UPDATE, so re-insert the aged event directly
        // through the maintenance path is not available; instead the test
        // relies on recorded_at set at insert time (see purge test).
    }

    #[test]
    fn audit_tables_reject_edits_outside_a_maintenance_window() {
        let dir = temp_dir();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        assert!(guards_present(&db));
        seed(&db, "s1", "2026-01-01T00:00:00+00:00");

        let conn = db.conn();
        // Triggers fire per row: give the tables without rows one each.
        conn.execute_batch(
            "INSERT INTO audit_recordings (recording_id, session_id, started_at, status, cols, rows, input_policy, format_version) VALUES ('r1', 's1', 'now', 'complete', 80, 24, 'metadata', 1);
             INSERT INTO audit_recording_chunks (recording_id, seq, start_ms, end_ms, path, size_bytes, sha256, compression, event_count) VALUES ('r1', 1, 0, 1, 'r1/1', 1, 'x', 'gzip', 1);
             INSERT INTO audit_logs (id, user_id, username, server_id, server_alias, server_host, session_id, connected_at) VALUES ('l1', 'u', 'u', 's', 's', 'h', 's1', 'now');",
        )
        .unwrap();
        let upd = conn.execute("UPDATE audit_events SET payload = '{}'", []);
        assert!(upd.unwrap_err().to_string().contains("append-only"));
        for table in GUARDED_DELETE {
            let del = conn.execute(&format!("DELETE FROM {}", table), []);
            assert!(
                del.unwrap_err().to_string().contains("retention policy"),
                "{} should be guarded",
                table
            );
        }
        // State transitions on sessions/operations remain possible.
        conn.execute(
            "UPDATE audit_operations SET status = 'succeeded' WHERE session_id = 's1'",
            [],
        )
        .unwrap();
        drop(conn);

        // Inside the window deletes work, and the window leaves no trace.
        let mut conn = db.conn();
        let n = with_maintenance_window(&mut conn, |tx| {
            tx.execute("DELETE FROM audit_events WHERE stream_id = 's1'", [])
                .map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(n, 1);
        let open: i64 = conn
            .query_row("SELECT COUNT(*) FROM audit_maintenance_window", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(open, 0);
        // A failing closure rolls back and closes the window too.
        let failed: Result<(), String> =
            with_maintenance_window(&mut conn, |_| Err("boom".to_string()));
        assert!(failed.is_err());
        assert!(conn
            .execute("DELETE FROM audit_sessions", [])
            .unwrap_err()
            .to_string()
            .contains("retention policy"));
    }

    #[test]
    fn retention_removes_only_finished_rows_past_the_cutoff_and_records_itself() {
        let dir = temp_dir();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        let old = "2020-01-01T00:00:00+00:00";
        seed(&db, "old", old);
        age(&db, "old", old);
        seed(&db, "live", "2026-01-01T00:00:00+00:00"); // still connected, op running
                                                        // Backdate the old session's event by inserting one with an explicit
                                                        // old recorded_at is impossible (assigned by the store), so simulate
                                                        // an old event stream through the maintenance window.
        {
            let mut conn = db.conn();
            with_maintenance_window(&mut conn, |tx| {
                tx.execute(
                    "INSERT INTO audit_events (event_id, schema_version, stream_id, seq, occurred_at, recorded_at, event_type, payload) VALUES ('e-old', 1, 'legacy', 1, ?1, ?1, 'test', '{}')",
                    params![old],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        }

        let report = purge_older_than(&db, RetentionPolicy { days: 30 }).unwrap();
        assert_eq!(report.sessions_removed, 1);
        assert_eq!(report.operations_removed, 1);
        assert_eq!(report.events_removed, 1, "only the backdated event");
        assert!(audit_events::get_session(&db, "old").unwrap().is_none());
        assert!(audit_events::get_session(&db, "live").unwrap().is_some());
        let (_, ops_live) = audit_events::list_operations(
            &db,
            &audit_events::OperationFilter {
                session_id: Some("live".into()),
                ..Default::default()
            },
            10,
            0,
        )
        .unwrap();
        assert_eq!(ops_live, 1);

        let sys = audit_events::recent_events(&db, MAINTENANCE_STREAM, 5).unwrap();
        assert_eq!(sys[0].event_type, RETENTION_EVENT);
        assert_eq!(sys[0].payload["sessionsRemoved"], 1);

        // Nothing left to purge: no second retention event.
        let again = purge_older_than(&db, RetentionPolicy { days: 30 }).unwrap();
        assert_eq!(
            again,
            PurgeReport {
                cutoff: again.cutoff.clone(),
                ..Default::default()
            }
        );
        assert_eq!(
            audit_events::recent_events(&db, MAINTENANCE_STREAM, 5)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn continuity_check_flags_holes_but_not_a_purged_prefix() {
        let dir = temp_dir();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        for _ in 0..4 {
            audit_events::append_event(
                &db,
                NewEvent {
                    stream_id: "a",
                    occurred_at: None,
                    session_id: None,
                    operation_id: None,
                    event_type: "t",
                    payload: json!({}),
                    integrity: Integrity::Complete,
                },
            )
            .unwrap();
        }
        let mut conn = db.conn();
        with_maintenance_window(&mut conn, |tx| {
            tx.execute(
                "DELETE FROM audit_events WHERE stream_id = 'a' AND seq IN (1, 3)",
                [],
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
        drop(conn);
        let (streams, gaps) = check_stream_continuity(&db).unwrap();
        assert_eq!(streams, 1);
        assert_eq!(
            gaps,
            vec![StreamGap {
                stream_id: "a".into(),
                after_seq: 2,
                next_seq: 4
            }]
        );
        let report = integrity_report(&db, None, 10).unwrap();
        assert!(report.append_only_guards_present);
        assert_eq!(report.stream_gaps.len(), 1);
        assert_eq!(report.recordings_checked, 0);
    }

    #[test]
    fn backup_snapshots_database_and_finished_chunks_only() {
        let dir = temp_dir();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        seed(&db, "s1", "2026-01-01T00:00:00+00:00");
        let root = dir.join("recordings");
        std::fs::create_dir_all(root.join("rec1")).unwrap();
        std::fs::write(root.join("rec1/000001.ndjson.gz"), b"chunk").unwrap();
        std::fs::write(root.join("rec1/000002.ndjson.gz.part"), b"half").unwrap();

        let dest = dir.join("backup");
        let report = backup(&db, &root, &dest).unwrap();
        assert_eq!(report.recordings_copied, 1);
        assert_eq!(report.chunk_files_copied, 1);
        assert_eq!(report.partial_files_skipped, 1);
        assert!(report.database_bytes > 0);
        assert!(dest.join("MANIFEST.json").exists());
        assert!(dest.join("recordings/rec1/000001.ndjson.gz").exists());
        assert!(!dest.join("recordings/rec1/000002.ndjson.gz.part").exists());

        // The copy is a working database with the same content and guards.
        let restored = Database::open(dest.to_str().unwrap()).unwrap();
        assert!(audit_events::get_session(&restored, "s1")
            .unwrap()
            .is_some());
        assert!(guards_present(&restored));
        assert_eq!(restored.schema_version(), db.schema_version());

        // Never overwrite an existing backup.
        assert!(backup(&db, &root, &dest).is_err());
    }
}
