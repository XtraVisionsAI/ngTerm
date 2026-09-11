//! Change preview and verification for file writes made through the
//! platform's own channel: the baseline (what the file is right now), a diff
//! for the person who approves or applies the change, a backup copy before
//! the write, and a read-back check afterwards. Every step leaves an audit
//! event on the operation so a failed verification keeps its evidence.
//!
//! What this does not claim: the remote host is not frozen between preview
//! and write. The baseline hash makes a change *detectable* (the write is
//! refused when the file differs from what was previewed), and the backup
//! makes a supported change *restorable*; neither makes arbitrary operations
//! reversible.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::audit_events::{self, Integrity, NewEvent};
use crate::db::Database;
use crate::AppState;

/// Stream that carries file-change events.
pub const STREAM: &str = "file_change";
pub const EVENT_BASELINE: &str = "file.baseline";
pub const EVENT_BACKUP: &str = "file.backup";
pub const EVENT_VERIFIED: &str = "file.verified";
pub const EVENT_VERIFY_FAILED: &str = "file.verify_failed";

/// Largest file that is read for a baseline hash. Above this the write is
/// still allowed but cannot be bound to a baseline or previewed.
pub const MAX_BASELINE_BYTES: u64 = 8 * 1024 * 1024;
/// Largest text that is diffed (per side).
pub const MAX_DIFF_BYTES: usize = 512 * 1024;
/// Diff lines kept in previews and snapshots.
pub const MAX_DIFF_LINES: usize = 400;
/// Line-pair budget for the LCS table; larger inputs get stats only.
const MAX_LCS_CELLS: usize = 4_000_000;

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Absolute path for `path` as the platform will address it, or `None` when
/// it cannot be determined server-side (home-relative without a known home,
/// relative without a working directory).
pub fn resolve_path(path: &str, cwd: Option<&str>) -> Option<String> {
    let p = path.trim();
    if p.is_empty() {
        return None;
    }
    if p.starts_with('/') {
        return Some(p.to_string());
    }
    if p.starts_with('~') {
        return None;
    }
    let cwd = cwd?.trim();
    if !cwd.starts_with('/') {
        return None;
    }
    Some(format!("{}/{}", cwd.trim_end_matches('/'), p))
}

/// The file as it is before a change.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub path: String,
    pub exists: bool,
    pub size: u64,
    /// SHA-256 of the current content; `None` when the file does not exist
    /// or is too large to hash.
    pub sha256: Option<String>,
    /// Current content when it is text within the diff limit.
    #[serde(skip)]
    pub text: Option<String>,
    pub binary: bool,
    pub too_large: bool,
}

impl Baseline {
    /// Value that goes into a summary / approval snapshot: hash for an
    /// existing file, a marker otherwise.
    pub fn binding(&self) -> String {
        match &self.sha256 {
            Some(h) => h.clone(),
            None if !self.exists => "new-file".into(),
            None => "unhashed".into(),
        }
    }

    pub fn short_binding(&self) -> String {
        let b = self.binding();
        if b.len() > 12 {
            b[..12].to_string()
        } else {
            b
        }
    }
}

/// Read the baseline of `path` through the session's helper connection.
/// A missing file is a valid baseline (`exists: false`); other errors are
/// returned so the caller does not treat an unreadable file as new.
pub async fn read_baseline(
    state: &AppState,
    session_id: &str,
    path: &str,
) -> Result<Baseline, String> {
    let meta = match state.helpers.sftp_stat(session_id, path).await {
        Ok(m) => m,
        Err(e) if is_not_found(&e) => {
            return Ok(Baseline {
                path: path.to_string(),
                exists: false,
                size: 0,
                sha256: None,
                text: None,
                binary: false,
                too_large: false,
            })
        }
        Err(e) => return Err(format!("cannot stat {}: {}", path, e)),
    };
    if meta.size > MAX_BASELINE_BYTES {
        return Ok(Baseline {
            path: path.to_string(),
            exists: true,
            size: meta.size,
            sha256: None,
            text: None,
            binary: false,
            too_large: true,
        });
    }
    let data = state
        .helpers
        .sftp_read(session_id, path)
        .await
        .map_err(|e| format!("cannot read {}: {}", path, e))?;
    let binary = data.contains(&0u8);
    let text = if !binary && data.len() <= MAX_DIFF_BYTES {
        String::from_utf8(data.clone()).ok()
    } else {
        None
    };
    Ok(Baseline {
        path: path.to_string(),
        exists: true,
        size: data.len() as u64,
        sha256: Some(sha256_hex(&data)),
        binary: binary || text.is_none() && data.len() <= MAX_DIFF_BYTES,
        text,
        too_large: false,
    })
}

fn is_not_found(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    l.contains("no such file")
        || l.contains("not found")
        || l.contains("nosuchfile")
        || l.contains("os error 2")
        || l.contains("(2)")
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiffPreview {
    pub added: usize,
    pub removed: usize,
    /// Unified-style lines (`+`, `-`, ` ` prefixed), context trimmed to 3.
    pub lines: Vec<String>,
    /// The diff was cut at [`MAX_DIFF_LINES`] or the inputs were too big to
    /// align; `added`/`removed` are then line-count estimates.
    pub truncated: bool,
}

/// Line diff of `old` → `new`. Deterministic for equal inputs so it can be
/// part of an approval snapshot.
pub fn unified_diff(old: &str, new: &str) -> DiffPreview {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len().saturating_mul(b.len()) > MAX_LCS_CELLS {
        return DiffPreview {
            added: b.len(),
            removed: a.len(),
            lines: vec![format!(
                "@@ diff too large to align: {} lines before, {} lines after @@",
                a.len(),
                b.len()
            )],
            truncated: true,
        };
    }
    // LCS table (a.len()+1) x (b.len()+1), u32 to keep memory in check.
    let w = b.len() + 1;
    let mut t = vec![0u32; (a.len() + 1) * w];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            t[i * w + j] = if a[i] == b[j] {
                t[(i + 1) * w + j + 1] + 1
            } else {
                t[(i + 1) * w + j].max(t[i * w + j + 1])
            };
        }
    }
    // Walk the table producing tagged lines.
    enum Op<'a> {
        Eq(&'a str),
        Del(&'a str),
        Add(&'a str),
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            ops.push(Op::Eq(a[i]));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            ops.push(Op::Del(a[i]));
            i += 1;
        } else {
            ops.push(Op::Add(b[j]));
            j += 1;
        }
    }
    while i < a.len() {
        ops.push(Op::Del(a[i]));
        i += 1;
    }
    while j < b.len() {
        ops.push(Op::Add(b[j]));
        j += 1;
    }
    // Keep 3 lines of context around changes; mark skipped runs.
    const CTX: usize = 3;
    let changed: Vec<bool> = ops.iter().map(|o| !matches!(o, Op::Eq(_))).collect();
    let mut keep = vec![false; ops.len()];
    for (k, &c) in changed.iter().enumerate() {
        if c {
            let lo = k.saturating_sub(CTX);
            let hi = (k + CTX + 1).min(ops.len());
            for item in keep.iter_mut().take(hi).skip(lo) {
                *item = true;
            }
        }
    }
    let mut out = Vec::new();
    let (mut added, mut removed) = (0usize, 0usize);
    let mut skipping = 0usize;
    let mut truncated = false;
    for (k, op) in ops.iter().enumerate() {
        match op {
            Op::Add(_) => added += 1,
            Op::Del(_) => removed += 1,
            Op::Eq(_) => {}
        }
        if !keep[k] {
            skipping += 1;
            continue;
        }
        if skipping > 0 {
            if out.len() < MAX_DIFF_LINES {
                out.push(format!("@@ {} unchanged lines @@", skipping));
            }
            skipping = 0;
        }
        if out.len() >= MAX_DIFF_LINES {
            truncated = true;
            continue;
        }
        out.push(match op {
            Op::Eq(l) => format!(" {}", l),
            Op::Del(l) => format!("-{}", l),
            Op::Add(l) => format!("+{}", l),
        });
    }
    if skipping > 0 && out.len() < MAX_DIFF_LINES {
        out.push(format!("@@ {} unchanged lines @@", skipping));
    }
    DiffPreview {
        added,
        removed,
        lines: out,
        truncated,
    }
}

/// Where the pre-write copy goes: a hidden sibling so listings stay clean
/// and the original directory's permissions apply.
pub fn backup_path_for(path: &str, stamp: &str) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(idx) => (&path[..idx], &path[idx + 1..]),
        None => ("", path),
    };
    let dir = if dir.is_empty() && path.starts_with('/') {
        "/"
    } else {
        dir
    };
    let sep = if dir.is_empty() || dir.ends_with('/') {
        ""
    } else {
        "/"
    };
    format!("{}{}.{}.ngterm-bak-{}", dir, sep, name, stamp)
}

pub fn backup_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Copy the current content of `path` next to it. Returns the backup path
/// and the hash of what was copied.
pub async fn backup_file(
    state: &AppState,
    session_id: &str,
    path: &str,
) -> Result<(String, String), String> {
    let data = state
        .helpers
        .sftp_read(session_id, path)
        .await
        .map_err(|e| format!("backup: cannot read {}: {}", path, e))?;
    let backup = backup_path_for(path, &backup_stamp());
    state
        .helpers
        .sftp_write(session_id, &backup, &data)
        .await
        .map_err(|e| format!("backup: cannot write {}: {}", backup, e))?;
    Ok((backup, sha256_hex(&data)))
}

/// Read `path` back and hash it.
pub async fn read_back_hash(
    state: &AppState,
    session_id: &str,
    path: &str,
) -> Result<String, String> {
    let data = state
        .helpers
        .sftp_read(session_id, path)
        .await
        .map_err(|e| format!("verify: cannot read {}: {}", path, e))?;
    Ok(sha256_hex(&data))
}

/// Append a file-change event to `operation_id`. Failures are logged; the
/// operation record itself is the primary evidence.
pub fn record_event(
    db: &Database,
    session_id: &str,
    operation_id: &str,
    event_type: &str,
    payload: serde_json::Value,
) {
    if let Err(e) = audit_events::append_event(
        db,
        NewEvent {
            stream_id: STREAM,
            occurred_at: None,
            session_id: Some(session_id),
            operation_id: Some(operation_id),
            event_type,
            payload,
            integrity: Integrity::Complete,
        },
    ) {
        tracing::error!(
            "file change event {} for operation {} not recorded: {}",
            event_type,
            operation_id,
            e
        );
    }
}

/// Outcome of a verified write, returned to the caller.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    pub operation_id: String,
    pub path: String,
    pub sha256: String,
    pub verified: bool,
    pub backup_path: Option<String>,
    pub baseline: Baseline,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_paths_only_when_unambiguous() {
        assert_eq!(resolve_path("/etc/x", None).as_deref(), Some("/etc/x"));
        assert_eq!(
            resolve_path("conf/x", Some("/srv/app/")).as_deref(),
            Some("/srv/app/conf/x")
        );
        assert_eq!(resolve_path("conf/x", None), None);
        assert_eq!(resolve_path("~/x", Some("/srv")), None);
        assert_eq!(resolve_path("x", Some("relative")), None);
    }

    #[test]
    fn diff_reports_changes_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\nI\n";
        let d = unified_diff(old, new);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 1);
        assert!(d.lines.contains(&"-d".to_string()));
        assert!(d.lines.contains(&"+D".to_string()));
        assert!(d.lines.contains(&"+I".to_string()));
        assert!(!d.truncated);
        // Deterministic.
        assert_eq!(d, unified_diff(old, new));
        let same = unified_diff(old, old);
        assert_eq!(same.added + same.removed, 0);
        assert_eq!(same.lines, vec!["@@ 8 unchanged lines @@".to_string()]);
    }

    #[test]
    fn diff_is_bounded() {
        let old: String = (0..3000).map(|i| format!("l{}\n", i)).collect();
        let new: String = (0..3000).map(|i| format!("m{}\n", i)).collect();
        let d = unified_diff(&old, &new);
        assert!(d.truncated);
        assert!(d.lines.len() <= MAX_DIFF_LINES);
        assert_eq!(d.added, 3000);
        assert_eq!(d.removed, 3000);
    }

    #[test]
    fn backup_paths_stay_in_the_directory() {
        assert_eq!(
            backup_path_for("/etc/nginx/nginx.conf", "S"),
            "/etc/nginx/.nginx.conf.ngterm-bak-S"
        );
        assert_eq!(backup_path_for("/top", "S"), "/.top.ngterm-bak-S");
        assert_eq!(backup_path_for("rel.txt", "S"), ".rel.txt.ngterm-bak-S");
    }

    #[test]
    fn baseline_binding_distinguishes_new_and_hashed() {
        let b = Baseline {
            path: "/x".into(),
            exists: false,
            size: 0,
            sha256: None,
            text: None,
            binary: false,
            too_large: false,
        };
        assert_eq!(b.binding(), "new-file");
        let b2 = Baseline {
            sha256: Some("abcdef0123456789".into()),
            exists: true,
            ..b
        };
        assert_eq!(b2.short_binding(), "abcdef012345");
    }
}
