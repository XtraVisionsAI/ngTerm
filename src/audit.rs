use chrono::Utc;
use serde::Serialize;

use crate::db::Database;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AuditLog {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub server_id: String,
    pub server_alias: String,
    pub server_host: String,
    pub session_id: String,
    pub connected_at: String,
    pub disconnected_at: Option<String>,
    pub duration_secs: Option<i64>,
    pub disconnect_reason: String,
}

pub struct AuditFilter {
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub server: Option<String>,
    pub status: Option<String>,
    pub time_from: Option<String>,
    pub time_to: Option<String>,
}

pub fn close_stale_sessions(db: &Database) {
    let now = Utc::now().to_rfc3339();
    let result = db.conn().execute(
        "UPDATE audit_logs SET disconnected_at = ?1, disconnect_reason = 'server_restart' WHERE disconnected_at IS NULL",
        rusqlite::params![now],
    );
    match result {
        Ok(n) if n > 0 => tracing::info!("Closed {} stale audit sessions from previous run", n),
        Err(e) => tracing::error!("Failed to close stale audit sessions: {}", e),
        _ => {}
    }
}

pub fn log_connect(
    db: &Database,
    user_id: &str,
    username: &str,
    server_id: &str,
    server_alias: &str,
    server_host: &str,
    session_id: &str,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    db.conn()
        .execute(
            "INSERT INTO audit_logs (id, user_id, username, server_id, server_alias, server_host, session_id, connected_at, disconnect_reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active')",
            rusqlite::params![id, user_id, username, server_id, server_alias, server_host, session_id, now],
        )
        .map_err(|e| e.to_string())?;

    Ok(id)
}

pub fn log_disconnect(db: &Database, session_id: &str, reason: &str) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();

    let conn = db.conn();
    let connected_at: Option<String> = conn
        .query_row(
            "SELECT connected_at FROM audit_logs WHERE session_id = ?1 AND disconnected_at IS NULL",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .ok();

    let duration_secs = connected_at.and_then(|ca| {
        chrono::DateTime::parse_from_rfc3339(&ca)
            .ok()
            .map(|start| (Utc::now() - start.with_timezone(&Utc)).num_seconds())
    });

    conn.execute(
        "UPDATE audit_logs SET disconnected_at = ?1, duration_secs = ?2, disconnect_reason = ?3 WHERE session_id = ?4 AND disconnected_at IS NULL",
        rusqlite::params![now, duration_secs, reason, session_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn list_logs(
    db: &Database,
    filter: &AuditFilter,
    limit: u32,
    offset: u32,
) -> Result<(Vec<AuditLog>, u32), String> {
    let conn = db.conn();

    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref uid) = filter.user_id {
        params.push(Box::new(uid.clone()));
        conditions.push(format!("user_id = ?{}", params.len()));
    }
    if let Some(ref uname) = filter.username {
        params.push(Box::new(format!("%{}%", uname)));
        conditions.push(format!("username LIKE ?{}", params.len()));
    }
    if let Some(ref server) = filter.server {
        params.push(Box::new(server.clone()));
        conditions.push(format!("server_alias = ?{}", params.len()));
    }
    if let Some(ref status) = filter.status {
        params.push(Box::new(status.clone()));
        conditions.push(format!("disconnect_reason = ?{}", params.len()));
    }
    if let Some(ref from) = filter.time_from {
        params.push(Box::new(from.clone()));
        conditions.push(format!("connected_at >= ?{}", params.len()));
    }
    if let Some(ref to) = filter.time_to {
        params.push(Box::new(to.clone()));
        conditions.push(format!("connected_at <= ?{}", params.len()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM audit_logs {}", where_clause);
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let total: u32 = conn
        .query_row(&count_sql, param_refs.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())?;

    params.push(Box::new(limit));
    let limit_idx = params.len();
    params.push(Box::new(offset));
    let offset_idx = params.len();

    let query_sql = format!(
        "SELECT id, user_id, username, server_id, server_alias, server_host, session_id, connected_at, disconnected_at, duration_secs, disconnect_reason FROM audit_logs {} ORDER BY connected_at DESC LIMIT ?{} OFFSET ?{}",
        where_clause, limit_idx, offset_idx
    );

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&query_sql).map_err(|e| e.to_string())?;

    let logs = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(AuditLog {
                id: row.get(0)?,
                user_id: row.get(1)?,
                username: row.get(2)?,
                server_id: row.get(3)?,
                server_alias: row.get(4)?,
                server_host: row.get(5)?,
                session_id: row.get(6)?,
                connected_at: row.get(7)?,
                disconnected_at: row.get(8)?,
                duration_secs: row.get(9)?,
                disconnect_reason: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok((logs, total))
}

pub fn list_filter_options(
    db: &Database,
    user_id: Option<&str>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let conn = db.conn();

    let usernames: Vec<String> = if user_id.is_none() {
        let mut stmt = conn
            .prepare("SELECT DISTINCT username FROM audit_logs ORDER BY username")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        vec![]
    };

    let servers: Vec<String> = if let Some(uid) = user_id {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT server_alias FROM audit_logs WHERE user_id = ?1 ORDER BY server_alias",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![uid], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn
            .prepare("SELECT DISTINCT server_alias FROM audit_logs ORDER BY server_alias")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };

    Ok((usernames, servers))
}
