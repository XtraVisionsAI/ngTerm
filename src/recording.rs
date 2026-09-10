//! Terminal session recording: capture (AUD-02) and chunked storage (AUD-04).
//!
//! Recording happens inside the session fan-out task, so it does not depend
//! on a browser being attached. Events are timestamped relative to the
//! recording start and appended to an in-memory chunk; chunks are
//! gzip-compressed, hashed and written to `<data_dir>/recordings/<id>/`
//! with their metadata in SQLite. The recorder never blocks the terminal:
//! when its queue is full, events are dropped and counted, and the gap is
//! written into the stream and reflected in the final integrity.
//!
//! Input is captured according to [`InputPolicy`]; the default keeps only
//! timing and byte counts so typed passwords never reach disk.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::audit_events::Integrity;
use crate::db::Database;

pub const RECORDING_FORMAT_VERSION: u32 = 1;
const COMPRESSION: &str = "gzip";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputPolicy {
    /// Do not record input at all.
    None,
    /// Record that input happened and how many bytes, never the content.
    Metadata,
    /// Record input bytes. May capture typed secrets.
    Content,
}

impl InputPolicy {
    fn as_str(self) -> &'static str {
        match self {
            InputPolicy::None => "none",
            InputPolicy::Metadata => "metadata",
            InputPolicy::Content => "content",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordingConfig {
    pub enabled: bool,
    pub root: PathBuf,
    pub input_policy: InputPolicy,
    /// Uncompressed event bytes after which a chunk is written out.
    pub chunk_max_bytes: usize,
    /// A non-empty chunk is written out after this long even if small.
    pub chunk_max_age: Duration,
    /// Events buffered between the session task and the writer; overflow
    /// is dropped and recorded as a gap.
    pub queue_capacity: usize,
    /// Cap on the total size of stored recordings. Enforced by deleting the
    /// oldest finished recordings; a live recording that still exceeds the
    /// cap is truncated, never silently continued.
    pub max_total_bytes: Option<u64>,
    /// Finished recordings older than this are deleted.
    pub retention_days: Option<u32>,
}

impl RecordingConfig {
    pub fn for_data_dir(data_dir: &str) -> Self {
        Self {
            enabled: true,
            root: Path::new(data_dir).join("recordings"),
            input_policy: InputPolicy::Metadata,
            chunk_max_bytes: 1024 * 1024,
            chunk_max_age: Duration::from_secs(30),
            queue_capacity: 1024,
            max_total_bytes: None,
            retention_days: None,
        }
    }

    /// Defaults overridden by `NGTERM_RECORDING` (`off`),
    /// `NGTERM_RECORDING_INPUT` (`none|metadata|content`),
    /// `NGTERM_RECORDING_MAX_MB` and `NGTERM_RECORDING_RETENTION_DAYS`.
    pub fn from_env(data_dir: &str) -> Self {
        let mut cfg = Self::for_data_dir(data_dir);
        if let Ok(v) = std::env::var("NGTERM_RECORDING") {
            cfg.enabled = !matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "false");
        }
        if let Ok(v) = std::env::var("NGTERM_RECORDING_INPUT") {
            cfg.input_policy = match v.to_ascii_lowercase().as_str() {
                "none" => InputPolicy::None,
                "content" => InputPolicy::Content,
                _ => InputPolicy::Metadata,
            };
        }
        if let Some(mb) = std::env::var("NGTERM_RECORDING_MAX_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.max_total_bytes = Some(mb * 1024 * 1024);
        }
        if let Some(days) = std::env::var("NGTERM_RECORDING_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            cfg.retention_days = Some(days);
        }
        cfg
    }
}

// --- Event model -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Output(Vec<u8>),
    Input {
        bytes: usize,
        data: Option<Vec<u8>>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// `dropped` events were lost before this point.
    Gap {
        dropped: u64,
    },
    Marker(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEvent {
    /// Milliseconds since the recording started.
    pub t_ms: u64,
    pub kind: EventKind,
}

impl RecordedEvent {
    fn to_line(&self) -> String {
        let mut v = serde_json::json!({ "t": self.t_ms });
        match &self.kind {
            EventKind::Output(d) => {
                v["k"] = "o".into();
                v["d"] = B64.encode(d).into();
            }
            EventKind::Input { bytes, data } => {
                v["k"] = "i".into();
                v["n"] = (*bytes as u64).into();
                if let Some(d) = data {
                    v["d"] = B64.encode(d).into();
                }
            }
            EventKind::Resize { cols, rows } => {
                v["k"] = "r".into();
                v["c"] = (*cols).into();
                v["r"] = (*rows).into();
            }
            EventKind::Gap { dropped } => {
                v["k"] = "g".into();
                v["n"] = (*dropped).into();
            }
            EventKind::Marker(m) => {
                v["k"] = "m".into();
                v["d"] = m.as_str().into();
            }
        }
        v.to_string()
    }

    fn from_line(line: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let t_ms = v["t"].as_u64().ok_or("event without timestamp")?;
        let b64 = |key: &str| -> Result<Option<Vec<u8>>, String> {
            match v[key].as_str() {
                Some(s) => B64.decode(s).map(Some).map_err(|e| e.to_string()),
                None => Ok(None),
            }
        };
        let kind = match v["k"].as_str() {
            Some("o") => EventKind::Output(b64("d")?.unwrap_or_default()),
            Some("i") => EventKind::Input {
                bytes: v["n"].as_u64().unwrap_or(0) as usize,
                data: b64("d")?,
            },
            Some("r") => EventKind::Resize {
                cols: v["c"].as_u64().unwrap_or(0) as u16,
                rows: v["r"].as_u64().unwrap_or(0) as u16,
            },
            Some("g") => EventKind::Gap {
                dropped: v["n"].as_u64().unwrap_or(0),
            },
            Some("m") => EventKind::Marker(v["d"].as_str().unwrap_or("").to_string()),
            other => return Err(format!("unknown event kind {:?}", other)),
        };
        Ok(Self { t_ms, kind })
    }
}

// --- Metadata --------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMeta {
    pub recording_id: String,
    pub session_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// `recording`, `complete` or `interrupted`.
    pub status: String,
    pub integrity: Integrity,
    pub cols: u16,
    pub rows: u16,
    pub input_policy: String,
    pub chunk_count: u32,
    pub total_bytes: u64,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkMeta {
    pub seq: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub compression: String,
    pub event_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChunkProblem {
    Missing {
        seq: u32,
    },
    Corrupt {
        seq: u32,
        detail: String,
    },
    /// A chunk sequence number is absent from the index.
    IndexGap {
        expected_seq: u32,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RetentionReport {
    pub removed_recordings: usize,
    pub removed_bytes: u64,
}

pub fn migration_v3_recordings(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS audit_recordings (
            recording_id     TEXT PRIMARY KEY,
            session_id       TEXT NOT NULL,
            started_at       TEXT NOT NULL,
            ended_at         TEXT,
            status           TEXT NOT NULL,
            integrity        TEXT NOT NULL DEFAULT 'complete',
            integrity_detail TEXT,
            cols             INTEGER NOT NULL,
            rows             INTEGER NOT NULL,
            input_policy     TEXT NOT NULL,
            format_version   INTEGER NOT NULL,
            chunk_count      INTEGER NOT NULL DEFAULT 0,
            total_bytes      INTEGER NOT NULL DEFAULT 0,
            duration_ms      INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_audit_recordings_session ON audit_recordings(session_id);
        CREATE INDEX IF NOT EXISTS idx_audit_recordings_ended ON audit_recordings(ended_at);
        CREATE TABLE IF NOT EXISTS audit_recording_chunks (
            recording_id TEXT NOT NULL,
            seq          INTEGER NOT NULL,
            start_ms     INTEGER NOT NULL,
            end_ms       INTEGER NOT NULL,
            path         TEXT NOT NULL,
            size_bytes   INTEGER NOT NULL,
            sha256       TEXT NOT NULL,
            compression  TEXT NOT NULL,
            event_count  INTEGER NOT NULL,
            PRIMARY KEY (recording_id, seq)
        );
        ",
    )
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn integrity_db(i: &Integrity) -> (&'static str, Option<String>) {
    match i {
        Integrity::Complete => ("complete", None),
        Integrity::Gap { dropped } => ("gap", Some(dropped.to_string())),
        Integrity::Truncated { reason } => ("truncated", Some(reason.clone())),
    }
}

fn integrity_from_db(kind: &str, detail: Option<String>) -> Integrity {
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

// --- Store -----------------------------------------------------------------

pub struct RecordingStore {
    db: Database,
    config: RecordingConfig,
}

impl RecordingStore {
    pub fn new(db: Database, config: RecordingConfig) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&config.root)
            .map_err(|e| format!("cannot create recording dir {:?}: {}", config.root, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&config.root, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Arc::new(Self { db, config }))
    }

    pub fn config(&self) -> &RecordingConfig {
        &self.config
    }

    /// Startup housekeeping: recordings still marked live belong to a
    /// previous process and are closed as interrupted; directories with no
    /// index entry and leftover partial chunk files are removed. Returns how
    /// many recordings were marked interrupted.
    pub fn recover(&self) -> Result<usize, String> {
        let n = self
            .db
            .conn()
            .execute(
                "UPDATE audit_recordings SET status = 'interrupted', integrity = 'truncated', integrity_detail = 'server restarted while recording', ended_at = ?1 WHERE status = 'recording'",
                params![now()],
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            tracing::warn!("{} recording(s) were interrupted by a previous shutdown", n);
        }

        let known: HashSet<String> = {
            let conn = self.db.conn();
            let mut stmt = conn
                .prepare("SELECT recording_id FROM audit_recordings")
                .map_err(|e| e.to_string())?;
            let ids = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect();
            ids
        };
        if let Ok(entries) = std::fs::read_dir(&self.config.root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                if path.is_dir() && !known.contains(&name) {
                    tracing::warn!("Removing orphan recording directory {:?}", path);
                    let _ = std::fs::remove_dir_all(&path);
                } else if path.is_dir() {
                    if let Ok(files) = std::fs::read_dir(&path) {
                        for f in files.flatten() {
                            if f.path().extension().and_then(|e| e.to_str()) == Some("part") {
                                let _ = std::fs::remove_file(f.path());
                            }
                        }
                    }
                }
            }
        }
        Ok(n)
    }

    /// Begin recording a session. Returns `None` when recording is disabled
    /// or the index row cannot be written; the caller then knows the session
    /// is unrecorded rather than assuming a recording exists.
    pub fn start(self: &Arc<Self>, session_id: &str, cols: u16, rows: u16) -> Option<Recorder> {
        if !self.config.enabled {
            return None;
        }
        let recording_id = uuid::Uuid::new_v4().to_string();
        let inserted = self.db.conn().execute(
            "INSERT INTO audit_recordings (recording_id, session_id, started_at, status, cols, rows, input_policy, format_version) VALUES (?1, ?2, ?3, 'recording', ?4, ?5, ?6, ?7)",
            params![
                recording_id,
                session_id,
                now(),
                cols,
                rows,
                self.config.input_policy.as_str(),
                RECORDING_FORMAT_VERSION
            ],
        );
        if let Err(e) = inserted {
            tracing::error!(
                "Recording for session {} not started: index write failed: {}",
                session_id,
                e
            );
            return None;
        }

        let (tx, rx) = mpsc::channel(self.config.queue_capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Writer {
            store: self.clone(),
            recording_id: recording_id.clone(),
            dropped: dropped.clone(),
            seq: 0,
            buf: Vec::new(),
            buf_events: 0,
            buf_start_ms: None,
            last_ms: 0,
            chunk_opened: Instant::now(),
            gap_total: 0,
            truncated: None,
            total_bytes: 0,
        };
        tokio::spawn(writer.run(rx));

        let recorder = Recorder {
            recording_id,
            tx,
            dropped,
            started: Instant::now(),
            policy: self.config.input_policy,
        };
        recorder.push(EventKind::Resize { cols, rows });
        Some(recorder)
    }

    pub fn total_bytes(&self) -> Result<u64, String> {
        self.db
            .conn()
            .query_row(
                "SELECT COALESCE(SUM(total_bytes), 0) FROM audit_recordings",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n.max(0) as u64)
            .map_err(|e| e.to_string())
    }

    pub fn get(&self, recording_id: &str) -> Result<Option<RecordingMeta>, String> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&format!("{} WHERE recording_id = ?1", Self::META_SELECT))
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![recording_id], Self::row_to_meta)
            .map_err(|e| e.to_string())?;
        rows.next().transpose().map_err(|e| e.to_string())
    }

    pub fn list_for_session(&self, session_id: &str) -> Result<Vec<RecordingMeta>, String> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare(&format!(
                "{} WHERE session_id = ?1 ORDER BY started_at",
                Self::META_SELECT
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], Self::row_to_meta)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    const META_SELECT: &'static str = "SELECT recording_id, session_id, started_at, ended_at, status, integrity, integrity_detail, cols, rows, input_policy, chunk_count, total_bytes, duration_ms FROM audit_recordings";

    fn row_to_meta(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingMeta> {
        Ok(RecordingMeta {
            recording_id: r.get(0)?,
            session_id: r.get(1)?,
            started_at: r.get(2)?,
            ended_at: r.get(3)?,
            status: r.get(4)?,
            integrity: integrity_from_db(&r.get::<_, String>(5)?, r.get(6)?),
            cols: r.get::<_, i64>(7)? as u16,
            rows: r.get::<_, i64>(8)? as u16,
            input_policy: r.get(9)?,
            chunk_count: r.get::<_, i64>(10)? as u32,
            total_bytes: r.get::<_, i64>(11)?.max(0) as u64,
            duration_ms: r.get::<_, Option<i64>>(12)?.map(|d| d.max(0) as u64),
        })
    }

    pub fn chunks(&self, recording_id: &str) -> Result<Vec<ChunkMeta>, String> {
        let conn = self.db.conn();
        let mut stmt = conn
            .prepare("SELECT seq, start_ms, end_ms, path, size_bytes, sha256, compression, event_count FROM audit_recording_chunks WHERE recording_id = ?1 ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![recording_id], |r| {
                Ok(ChunkMeta {
                    seq: r.get::<_, i64>(0)? as u32,
                    start_ms: r.get::<_, i64>(1)?.max(0) as u64,
                    end_ms: r.get::<_, i64>(2)?.max(0) as u64,
                    path: r.get(3)?,
                    size_bytes: r.get::<_, i64>(4)?.max(0) as u64,
                    sha256: r.get(5)?,
                    compression: r.get(6)?,
                    event_count: r.get::<_, i64>(7)? as u32,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Read and verify one chunk against its index entry.
    pub fn read_chunk(&self, chunk: &ChunkMeta) -> Result<Vec<RecordedEvent>, String> {
        let path = self.config.root.join(&chunk.path);
        let compressed = std::fs::read(&path).map_err(|e| format!("read {:?}: {}", path, e))?;
        if compressed.len() as u64 != chunk.size_bytes {
            return Err(format!(
                "size mismatch: index {} bytes, file {} bytes",
                chunk.size_bytes,
                compressed.len()
            ));
        }
        let digest = hex::encode(Sha256::digest(&compressed));
        if digest != chunk.sha256 {
            return Err("sha256 mismatch".to_string());
        }
        let text = decompress(&compressed)?;
        let mut lines = text.lines();
        let header = lines.next().ok_or("empty chunk")?;
        let header: serde_json::Value =
            serde_json::from_str(header).map_err(|e| format!("bad chunk header: {}", e))?;
        if header["v"].as_u64() != Some(RECORDING_FORMAT_VERSION as u64) {
            return Err(format!("unsupported chunk format {}", header["v"]));
        }
        let events = lines
            .filter(|l| !l.is_empty())
            .map(RecordedEvent::from_line)
            .collect::<Result<Vec<_>, _>>()?;
        if events.len() != chunk.event_count as usize {
            return Err(format!(
                "event count mismatch: index {}, chunk {}",
                chunk.event_count,
                events.len()
            ));
        }
        Ok(events)
    }

    /// Every event of a recording in order. Fails on the first missing or
    /// corrupt chunk instead of returning a silently incomplete stream; use
    /// [`Self::verify`] to enumerate problems.
    pub fn read_events(&self, recording_id: &str) -> Result<Vec<RecordedEvent>, String> {
        let mut out = Vec::new();
        for chunk in self.chunks(recording_id)? {
            out.extend(
                self.read_chunk(&chunk)
                    .map_err(|e| format!("chunk {}: {}", chunk.seq, e))?,
            );
        }
        Ok(out)
    }

    pub fn verify(&self, recording_id: &str) -> Result<Vec<ChunkProblem>, String> {
        let mut problems = Vec::new();
        let mut expected = 1u32;
        for chunk in self.chunks(recording_id)? {
            while expected < chunk.seq {
                problems.push(ChunkProblem::IndexGap {
                    expected_seq: expected,
                });
                expected += 1;
            }
            expected = chunk.seq + 1;
            let path = self.config.root.join(&chunk.path);
            if !path.exists() {
                problems.push(ChunkProblem::Missing { seq: chunk.seq });
                continue;
            }
            if let Err(detail) = self.read_chunk(&chunk) {
                problems.push(ChunkProblem::Corrupt {
                    seq: chunk.seq,
                    detail,
                });
            }
        }
        Ok(problems)
    }

    /// Apply the retention policy: delete finished recordings past
    /// `retention_days`, then the oldest finished recordings until the total
    /// is within `max_total_bytes`. Files are removed before their index
    /// rows so a crash leaves detectable missing chunks, never dangling files
    /// that look like valid history.
    pub fn enforce_retention(&self) -> Result<RetentionReport, String> {
        let mut report = RetentionReport::default();
        if let Some(days) = self.config.retention_days {
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
            let expired: Vec<String> = {
                let conn = self.db.conn();
                let mut stmt = conn
                    .prepare("SELECT recording_id FROM audit_recordings WHERE status != 'recording' AND ended_at IS NOT NULL AND ended_at < ?1")
                    .map_err(|e| e.to_string())?;
                let ids = stmt
                    .query_map(params![cutoff], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .collect();
                ids
            };
            for id in expired {
                report.removed_bytes += self.purge(&id)?;
                report.removed_recordings += 1;
            }
        }
        if let Some(cap) = self.config.max_total_bytes {
            while self.total_bytes()? > cap {
                let oldest: Option<String> = self
                    .db
                    .conn()
                    .query_row(
                        "SELECT recording_id FROM audit_recordings WHERE status != 'recording' ORDER BY ended_at ASC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .ok();
                match oldest {
                    Some(id) => {
                        report.removed_bytes += self.purge(&id)?;
                        report.removed_recordings += 1;
                    }
                    None => break,
                }
            }
        }
        Ok(report)
    }

    /// Delete one recording's files and index rows. Returns bytes freed.
    pub fn purge(&self, recording_id: &str) -> Result<u64, String> {
        let chunks = self.chunks(recording_id)?;
        let mut freed = 0;
        for c in &chunks {
            match std::fs::remove_file(self.config.root.join(&c.path)) {
                Ok(()) => freed += c.size_bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("remove {}: {}", c.path, e)),
            }
        }
        let _ = std::fs::remove_dir(self.config.root.join(recording_id));
        let mut conn = self.db.conn();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM audit_recording_chunks WHERE recording_id = ?1",
            params![recording_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM audit_recordings WHERE recording_id = ?1",
            params![recording_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(freed)
    }

    /// Run `enforce_retention` periodically for the life of the process.
    pub fn spawn_retention_task(self: Arc<Self>, every: Duration) {
        if self.config.retention_days.is_none() && self.config.max_total_bytes.is_none() {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            loop {
                interval.tick().await;
                let store = self.clone();
                let result = tokio::task::spawn_blocking(move || store.enforce_retention()).await;
                match result {
                    Ok(Ok(r)) if r.removed_recordings > 0 => tracing::info!(
                        "Recording retention removed {} recording(s), {} bytes",
                        r.removed_recordings,
                        r.removed_bytes
                    ),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::error!("Recording retention failed: {}", e),
                    Err(e) => tracing::error!("Recording retention task panicked: {}", e),
                }
            }
        });
    }

    fn insert_chunk(&self, recording_id: &str, row: &ChunkMeta) -> Result<(), String> {
        let mut conn = self.db.conn();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO audit_recording_chunks (recording_id, seq, start_ms, end_ms, path, size_bytes, sha256, compression, event_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                recording_id,
                row.seq,
                row.start_ms as i64,
                row.end_ms as i64,
                row.path,
                row.size_bytes as i64,
                row.sha256,
                row.compression,
                row.event_count
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE audit_recordings SET chunk_count = chunk_count + 1, total_bytes = total_bytes + ?1 WHERE recording_id = ?2",
            params![row.size_bytes as i64, recording_id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    fn finish_recording(
        &self,
        recording_id: &str,
        integrity: &Integrity,
        duration_ms: u64,
    ) -> Result<(), String> {
        let (kind, detail) = integrity_db(integrity);
        self.db
            .conn()
            .execute(
                "UPDATE audit_recordings SET status = 'complete', ended_at = ?1, integrity = ?2, integrity_detail = ?3, duration_ms = ?4 WHERE recording_id = ?5",
                params![now(), kind, detail, duration_ms as i64, recording_id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn decompress(bytes: &[u8]) -> Result<String, String> {
    use std::io::Read;
    let mut out = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut out)
        .map_err(|e| format!("decompress: {}", e))?;
    Ok(out)
}

/// Compress, hash and atomically place one chunk file. Returns the index row.
fn write_chunk_file(
    root: &Path,
    recording_id: &str,
    seq: u32,
    start_ms: u64,
    end_ms: u64,
    event_count: u32,
    body: &[u8],
) -> Result<ChunkMeta, String> {
    let dir = root.join(recording_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {:?}: {}", dir, e))?;
    let header = serde_json::json!({
        "v": RECORDING_FORMAT_VERSION,
        "recordingId": recording_id,
        "seq": seq,
        "startMs": start_ms,
    })
    .to_string();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(header.as_bytes())
        .and_then(|_| enc.write_all(b"\n"))
        .and_then(|_| enc.write_all(body))
        .map_err(|e| format!("compress: {}", e))?;
    let compressed = enc.finish().map_err(|e| format!("compress: {}", e))?;
    let sha256 = hex::encode(Sha256::digest(&compressed));

    let file_name = format!("{:06}.ndjson.gz", seq);
    let final_path = dir.join(&file_name);
    let tmp_path = dir.join(format!("{:06}.part", seq));
    std::fs::write(&tmp_path, &compressed).map_err(|e| format!("write {:?}: {}", tmp_path, e))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("rename {:?}: {}", final_path, e)
    })?;

    Ok(ChunkMeta {
        seq,
        start_ms,
        end_ms,
        path: format!("{}/{}", recording_id, file_name),
        size_bytes: compressed.len() as u64,
        sha256,
        compression: COMPRESSION.to_string(),
        event_count,
    })
}

// --- Recorder handle -------------------------------------------------------

enum Msg {
    Event(RecordedEvent),
    Finish(oneshot::Sender<Integrity>),
}

/// Producer side of one recording. Cheap to clone; all methods are
/// non-blocking and never fail visibly to the terminal path.
#[derive(Clone)]
pub struct Recorder {
    recording_id: String,
    tx: mpsc::Sender<Msg>,
    dropped: Arc<AtomicU64>,
    started: Instant,
    policy: InputPolicy,
}

impl Recorder {
    pub fn recording_id(&self) -> &str {
        &self.recording_id
    }

    pub fn output(&self, bytes: &[u8]) {
        self.push(EventKind::Output(bytes.to_vec()));
    }

    pub fn input(&self, bytes: &[u8]) {
        match self.policy {
            InputPolicy::None => {}
            InputPolicy::Metadata => self.push(EventKind::Input {
                bytes: bytes.len(),
                data: None,
            }),
            InputPolicy::Content => self.push(EventKind::Input {
                bytes: bytes.len(),
                data: Some(bytes.to_vec()),
            }),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.push(EventKind::Resize { cols, rows });
    }

    pub fn marker(&self, text: &str) {
        self.push(EventKind::Marker(text.to_string()));
    }

    fn push(&self, kind: EventKind) {
        let ev = RecordedEvent {
            t_ms: self.started.elapsed().as_millis() as u64,
            kind,
        };
        if self.tx.try_send(Msg::Event(ev)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Flush everything and close the recording. The returned integrity is
    /// what the writer actually achieved, not what was attempted.
    pub async fn finish(self) -> Integrity {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Finish(tx)).await.is_err() {
            return Integrity::Truncated {
                reason: "recorder task exited before the session ended".into(),
            };
        }
        rx.await.unwrap_or(Integrity::Truncated {
            reason: "recorder task exited before confirming the final chunk".into(),
        })
    }
}

// --- Writer task -----------------------------------------------------------

struct Writer {
    store: Arc<RecordingStore>,
    recording_id: String,
    dropped: Arc<AtomicU64>,
    seq: u32,
    buf: Vec<u8>,
    buf_events: u32,
    buf_start_ms: Option<u64>,
    last_ms: u64,
    chunk_opened: Instant,
    gap_total: u64,
    truncated: Option<String>,
    total_bytes: u64,
}

impl Writer {
    async fn run(mut self, mut rx: mpsc::Receiver<Msg>) {
        let age = self
            .store
            .config
            .chunk_max_age
            .max(Duration::from_millis(10));
        let mut ticker = tokio::time::interval(age);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reply: Option<oneshot::Sender<Integrity>> = None;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(Msg::Event(ev)) => {
                        // Drops only happen while the queue is full, so
                        // everything already queued precedes them: drain
                        // first, then place the gap marker.
                        self.ingest(ev).await;
                        let mut finish = None;
                        while let Ok(msg) = rx.try_recv() {
                            match msg {
                                Msg::Event(ev) => self.ingest(ev).await,
                                Msg::Finish(tx) => {
                                    finish = Some(tx);
                                    break;
                                }
                            }
                        }
                        self.note_drops();
                        if finish.is_some() {
                            reply = finish;
                            break;
                        }
                    }
                    Some(Msg::Finish(tx)) => { reply = Some(tx); break; }
                    None => break,
                },
                _ = ticker.tick() => {
                    if !self.buf.is_empty() && self.chunk_opened.elapsed() >= age {
                        self.flush().await;
                    }
                }
            }
        }
        self.note_drops();
        self.flush().await;
        let integrity = match &self.truncated {
            Some(reason) => Integrity::Truncated {
                reason: reason.clone(),
            },
            None if self.gap_total > 0 => Integrity::Gap {
                dropped: self.gap_total,
            },
            None => Integrity::Complete,
        };
        if let Err(e) = self
            .store
            .finish_recording(&self.recording_id, &integrity, self.last_ms)
        {
            tracing::error!(
                "Recording {} could not be closed in the index: {}",
                self.recording_id,
                e
            );
        }
        if let Some(tx) = reply {
            let _ = tx.send(integrity);
        }
    }

    async fn ingest(&mut self, ev: RecordedEvent) {
        self.append(ev);
        if self.buf.len() >= self.store.config.chunk_max_bytes {
            self.flush().await;
        }
    }

    fn note_drops(&mut self) {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            self.gap_total += dropped;
            tracing::warn!(
                "Recording {} dropped {} event(s): writer fell behind",
                self.recording_id,
                dropped
            );
            let t_ms = self.last_ms;
            self.append(RecordedEvent {
                t_ms,
                kind: EventKind::Gap { dropped },
            });
        }
    }

    fn append(&mut self, ev: RecordedEvent) {
        if self.truncated.is_some() {
            return;
        }
        if self.buf.is_empty() {
            self.buf_start_ms = Some(ev.t_ms);
            self.chunk_opened = Instant::now();
        }
        self.last_ms = self.last_ms.max(ev.t_ms);
        self.buf.extend_from_slice(ev.to_line().as_bytes());
        self.buf.push(b'\n');
        self.buf_events += 1;
    }

    async fn flush(&mut self) {
        if self.buf.is_empty() || self.truncated.is_some() {
            return;
        }
        let body = std::mem::take(&mut self.buf);
        let event_count = std::mem::take(&mut self.buf_events);
        let start_ms = self.buf_start_ms.take().unwrap_or(self.last_ms);
        let end_ms = self.last_ms;
        self.seq += 1;
        let seq = self.seq;
        let root = self.store.config.root.clone();
        let rid = self.recording_id.clone();

        let written = tokio::task::spawn_blocking(move || {
            write_chunk_file(&root, &rid, seq, start_ms, end_ms, event_count, &body)
        })
        .await
        .unwrap_or_else(|e| Err(format!("chunk writer panicked: {}", e)));

        let chunk = match written {
            Ok(c) => c,
            Err(e) => {
                self.truncate(format!("chunk {} not written: {}", seq, e));
                return;
            }
        };

        if let Some(cap) = self.store.config.max_total_bytes {
            let over = |store: &RecordingStore| {
                store
                    .total_bytes()
                    .map(|t| t + chunk.size_bytes > cap)
                    .unwrap_or(true)
            };
            if over(&self.store) {
                let store = self.store.clone();
                let _ = tokio::task::spawn_blocking(move || store.enforce_retention()).await;
            }
            if over(&self.store) {
                let _ = std::fs::remove_file(self.store.config.root.join(&chunk.path));
                self.truncate(format!("storage cap of {} bytes reached", cap));
                return;
            }
        }

        if let Err(e) = self.store.insert_chunk(&self.recording_id, &chunk) {
            let _ = std::fs::remove_file(self.store.config.root.join(&chunk.path));
            self.truncate(format!("chunk {} not indexed: {}", seq, e));
            return;
        }
        self.total_bytes += chunk.size_bytes;
    }

    fn truncate(&mut self, reason: String) {
        tracing::error!(
            "Recording {} stopped: {} (session continues unrecorded)",
            self.recording_id,
            reason
        );
        self.truncated = Some(reason);
        self.buf.clear();
        self.buf_events = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        store: Arc<RecordingStore>,
        dir: PathBuf,
    }

    fn fixture(tweak: impl FnOnce(&mut RecordingConfig)) -> Fx {
        let dir = std::env::temp_dir().join(format!("ngterm-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        let mut cfg = RecordingConfig::for_data_dir(dir.to_str().unwrap());
        cfg.chunk_max_age = Duration::from_secs(3600);
        tweak(&mut cfg);
        let store = RecordingStore::new(db, cfg).unwrap();
        Fx { store, dir }
    }

    fn kinds(events: &[RecordedEvent]) -> String {
        events
            .iter()
            .map(|e| match &e.kind {
                EventKind::Output(_) => "o",
                EventKind::Input { .. } => "i",
                EventKind::Resize { .. } => "r",
                EventKind::Gap { .. } => "g",
                EventKind::Marker(_) => "m",
            })
            .collect()
    }

    #[tokio::test]
    async fn records_output_input_and_resize_with_metadata_only_input() {
        let fx = fixture(|_| {});
        let rec = fx.store.start("s1", 80, 24).unwrap();
        rec.output(b"$ ");
        rec.input(b"secret-password\r");
        rec.resize(100, 30);
        rec.output(b"\r\nok\r\n");
        let rid = rec.recording_id().to_string();
        assert_eq!(rec.finish().await, Integrity::Complete);

        let meta = fx.store.get(&rid).unwrap().unwrap();
        assert_eq!(meta.status, "complete");
        assert_eq!(meta.integrity, Integrity::Complete);
        assert_eq!(meta.chunk_count, 1);
        assert_eq!(meta.input_policy, "metadata");
        assert!(meta.total_bytes > 0);

        let events = fx.store.read_events(&rid).unwrap();
        assert_eq!(kinds(&events), "roiro");
        assert_eq!(
            events[2].kind,
            EventKind::Input {
                bytes: 16,
                data: None
            }
        );
        assert_eq!(events[0].kind, EventKind::Resize { cols: 80, rows: 24 });
        assert_eq!(events[4].kind, EventKind::Output(b"\r\nok\r\n".to_vec()));
        // The typed secret must not be anywhere on disk.
        for chunk in fx.store.chunks(&rid).unwrap() {
            let raw = std::fs::read(fx.dir.join("recordings").join(&chunk.path)).unwrap();
            let text = decompress(&raw).unwrap();
            assert!(!text.contains(&B64.encode(b"secret-password\r")));
        }
        assert!(fx.store.verify(&rid).unwrap().is_empty());
    }

    #[tokio::test]
    async fn chunks_rotate_and_corruption_is_detected() {
        let fx = fixture(|c| c.chunk_max_bytes = 200);
        let rec = fx.store.start("s1", 80, 24).unwrap();
        for i in 0..20 {
            rec.output(format!("line {:02} {}\r\n", i, "x".repeat(40)).as_bytes());
        }
        let rid = rec.recording_id().to_string();
        assert_eq!(rec.finish().await, Integrity::Complete);

        let chunks = fx.store.chunks(&rid).unwrap();
        assert!(chunks.len() >= 3, "expected rotation, got {}", chunks.len());
        assert_eq!(
            chunks.iter().map(|c| c.seq).collect::<Vec<_>>(),
            (1..=chunks.len() as u32).collect::<Vec<_>>()
        );
        let events = fx.store.read_events(&rid).unwrap();
        assert_eq!(events.len(), 21);
        assert_eq!(
            events[1].kind,
            EventKind::Output(
                b"line 00 "
                    .iter()
                    .chain(b"x".repeat(40).iter())
                    .chain(b"\r\n")
                    .copied()
                    .collect()
            )
        );
        assert_eq!(
            fx.store.get(&rid).unwrap().unwrap().total_bytes,
            chunks.iter().map(|c| c.size_bytes).sum::<u64>()
        );

        // Flip a byte in chunk 2, delete chunk 3.
        let p2 = fx.dir.join("recordings").join(&chunks[1].path);
        let mut bytes = std::fs::read(&p2).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&p2, bytes).unwrap();
        std::fs::remove_file(fx.dir.join("recordings").join(&chunks[2].path)).unwrap();

        let problems = fx.store.verify(&rid).unwrap();
        assert!(
            matches!(problems[0], ChunkProblem::Corrupt { seq: 2, .. }),
            "{:?}",
            problems
        );
        assert_eq!(problems[1], ChunkProblem::Missing { seq: 3 });
        assert!(fx.store.read_events(&rid).is_err());
    }

    #[tokio::test]
    async fn writer_backlog_becomes_a_recorded_gap_not_a_stall() {
        // current_thread runtime: the writer cannot run until we await, so
        // the queue overflows deterministically.
        let fx = fixture(|c| c.queue_capacity = 2);
        let rec = fx.store.start("s1", 80, 24).unwrap(); // resize occupies one slot
        for i in 0..5u8 {
            rec.output(&[b'0' + i]);
        }
        let rid = rec.recording_id().to_string();
        assert_eq!(rec.finish().await, Integrity::Gap { dropped: 4 });
        let events = fx.store.read_events(&rid).unwrap();
        assert_eq!(kinds(&events), "rog");
        assert_eq!(events[2].kind, EventKind::Gap { dropped: 4 });
        assert_eq!(
            fx.store.get(&rid).unwrap().unwrap().integrity,
            Integrity::Gap { dropped: 4 }
        );
    }

    #[tokio::test]
    async fn restart_marks_live_recordings_interrupted_and_sweeps_orphans() {
        let fx = fixture(|_| {});
        let rec = fx.store.start("s1", 80, 24).unwrap();
        rec.output(b"hello");
        let rid = rec.recording_id().to_string();
        drop(rec); // process dies without finishing
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Whatever the writer managed, the row must not be left "recording"
        // after a restart. Force the pre-restart state to be sure.
        fx.store
            .db
            .conn()
            .execute(
                "UPDATE audit_recordings SET status = 'recording', ended_at = NULL WHERE recording_id = ?1",
                params![rid],
            )
            .unwrap();
        let orphan = fx.dir.join("recordings").join("not-in-index");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("000001.ndjson.gz"), b"junk").unwrap();

        let reopened = RecordingStore::new(fx.store.db.clone(), fx.store.config.clone()).unwrap();
        assert_eq!(reopened.recover().unwrap(), 1);
        let meta = reopened.get(&rid).unwrap().unwrap();
        assert_eq!(meta.status, "interrupted");
        assert!(matches!(meta.integrity, Integrity::Truncated { .. }));
        assert!(meta.ended_at.is_some());
        assert!(!orphan.exists());
    }

    #[tokio::test]
    async fn retention_removes_files_and_index_together_but_keeps_live_recordings() {
        let fx = fixture(|c| c.max_total_bytes = Some(1));
        let old = fx.store.start("s-old", 80, 24).unwrap();
        old.output(b"old data");
        let old_id = old.recording_id().to_string();
        // Cap enforcement during flush must not delete the recording that is
        // being written; nothing else is finished yet, so it is truncated.
        assert!(matches!(old.finish().await, Integrity::Truncated { .. }));

        let fx = fixture(|c| c.max_total_bytes = Some(10_000));
        let old = fx.store.start("s-old", 80, 24).unwrap();
        old.output(b"old data");
        let old_id2 = old.recording_id().to_string();
        assert_eq!(old.finish().await, Integrity::Complete);
        let live = fx.store.start("s-live", 80, 24).unwrap();
        let live_id = live.recording_id().to_string();

        let old_chunks = fx.store.chunks(&old_id2).unwrap();
        assert_eq!(old_chunks.len(), 1);
        let old_path = fx.dir.join("recordings").join(&old_chunks[0].path);
        assert!(old_path.exists());

        // Tighten the cap below what is stored and enforce.
        let mut cfg = fx.store.config.clone();
        cfg.max_total_bytes = Some(1);
        let tight = RecordingStore::new(fx.store.db.clone(), cfg).unwrap();
        let report = tight.enforce_retention().unwrap();
        assert_eq!(report.removed_recordings, 1);
        assert!(report.removed_bytes > 0);
        assert!(!old_path.exists());
        assert!(tight.get(&old_id2).unwrap().is_none());
        assert!(tight.chunks(&old_id2).unwrap().is_empty());
        assert!(tight.get(&live_id).unwrap().is_some());
        assert_eq!(live.finish().await, Integrity::Complete);
        let _ = old_id;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unwritable_storage_truncates_instead_of_claiming_complete() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores directory permissions
        }
        use std::os::unix::fs::PermissionsExt;
        let fx = fixture(|_| {});
        let rec = fx.store.start("s1", 80, 24).unwrap();
        let root = fx.dir.join("recordings");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
        rec.output(b"this will not be stored");
        let rid = rec.recording_id().to_string();
        let integrity = rec.finish().await;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(integrity, Integrity::Truncated { .. }),
            "{:?}",
            integrity
        );
        let meta = fx.store.get(&rid).unwrap().unwrap();
        assert_eq!(meta.status, "complete");
        assert!(matches!(meta.integrity, Integrity::Truncated { .. }));
        assert_eq!(meta.chunk_count, 0);
    }

    #[test]
    fn event_lines_round_trip() {
        let evs = vec![
            RecordedEvent {
                t_ms: 0,
                kind: EventKind::Resize { cols: 80, rows: 24 },
            },
            RecordedEvent {
                t_ms: 5,
                kind: EventKind::Output(vec![0, 27, b'[', 255]),
            },
            RecordedEvent {
                t_ms: 7,
                kind: EventKind::Input {
                    bytes: 3,
                    data: Some(b"ls\n".to_vec()),
                },
            },
            RecordedEvent {
                t_ms: 9,
                kind: EventKind::Gap { dropped: 12 },
            },
            RecordedEvent {
                t_ms: 11,
                kind: EventKind::Marker("注释".into()),
            },
        ];
        for ev in evs {
            assert_eq!(RecordedEvent::from_line(&ev.to_line()).unwrap(), ev);
        }
        assert!(RecordedEvent::from_line(r#"{"t":1,"k":"z"}"#).is_err());
    }
}
