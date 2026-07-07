use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub group_name: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub key_id: Option<String>,
    pub tags: Vec<String>,
    pub ai_tool_id: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub idle_timeout_secs: u32,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateServerRequest {
    pub group_name: Option<String>,
    pub alias: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub key_id: Option<String>,
    pub tags: Option<Vec<String>>,
    pub ai_tool_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServerRequest {
    pub group_name: Option<String>,
    pub alias: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub key_id: Option<String>,
    pub tags: Option<Vec<String>>,
    pub ai_tool_id: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub idle_timeout_secs: Option<u32>,
}

pub fn list_groups(db: &Database) -> Result<Vec<String>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT group_name FROM servers WHERE group_name != '' ORDER BY group_name",
        )
        .map_err(|e| e.to_string())?;

    let groups = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(groups)
}

pub fn create_server(db: &Database, req: &CreateServerRequest) -> Result<Server, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let port = req.port.unwrap_or(22);
    let group_name = req.group_name.as_deref().unwrap_or("");
    let tags = req.tags.clone().unwrap_or_default();
    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());

    db.conn()
        .execute(
            "INSERT INTO servers (id, group_name, alias, host, port, username, key_id, tags, ai_tool_id, idle_timeout_secs, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![id, group_name, req.alias, req.host, port, req.username, req.key_id, tags_json, req.ai_tool_id, 0_i32, now],
        )
        .map_err(|e| e.to_string())?;

    Ok(Server {
        id,
        group_name: group_name.to_string(),
        alias: req.alias.clone(),
        host: req.host.clone(),
        port,
        username: req.username.clone(),
        key_id: req.key_id.clone(),
        tags,
        ai_tool_id: req.ai_tool_id.clone(),
        host_key_fingerprint: None,
        idle_timeout_secs: 0,
        created_at: now,
    })
}

fn parse_server_row(row: &rusqlite::Row) -> rusqlite::Result<Server> {
    let tags_str: String = row.get(7)?;
    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
    Ok(Server {
        id: row.get(0)?,
        group_name: row.get(1)?,
        alias: row.get(2)?,
        host: row.get(3)?,
        port: row.get::<_, i32>(4)? as u16,
        username: row.get(5)?,
        key_id: row.get(6)?,
        tags,
        ai_tool_id: row.get(8)?,
        host_key_fingerprint: row.get(9)?,
        idle_timeout_secs: row.get::<_, i32>(10).unwrap_or(0) as u32,
        created_at: row.get(11)?,
    })
}

pub fn list_servers(db: &Database, group_name: Option<&str>) -> Result<Vec<Server>, String> {
    let conn = db.conn();
    let (sql, params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(g) = group_name {
        (
            "SELECT id, group_name, alias, host, port, username, key_id, tags, ai_tool_id, host_key_fingerprint, idle_timeout_secs, created_at FROM servers WHERE group_name = ?1",
            vec![Box::new(g.to_string())],
        )
    } else {
        (
            "SELECT id, group_name, alias, host, port, username, key_id, tags, ai_tool_id, host_key_fingerprint, idle_timeout_secs, created_at FROM servers",
            vec![],
        )
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let servers = stmt
        .query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            parse_server_row,
        )
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(servers)
}

pub fn get_server(db: &Database, id: &str) -> Result<Option<Server>, String> {
    let conn = db.conn();
    let result = conn.query_row(
        "SELECT id, group_name, alias, host, port, username, key_id, tags, ai_tool_id, host_key_fingerprint, idle_timeout_secs, created_at FROM servers WHERE id = ?1",
        rusqlite::params![id],
        parse_server_row,
    );

    match result {
        Ok(server) => Ok(Some(server)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn delete_server(db: &Database, id: &str) -> Result<bool, String> {
    let conn = db.conn();
    let affected = conn
        .execute("DELETE FROM servers WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

pub fn update_server(db: &Database, id: &str, req: &UpdateServerRequest) -> Result<bool, String> {
    let conn = db.conn();
    let mut sets = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ref group_name) = req.group_name {
        sets.push("group_name = ?");
        params.push(Box::new(group_name.clone()));
    }
    if let Some(ref alias) = req.alias {
        sets.push("alias = ?");
        params.push(Box::new(alias.clone()));
    }
    if let Some(ref host) = req.host {
        sets.push("host = ?");
        params.push(Box::new(host.clone()));
    }
    if let Some(port) = req.port {
        sets.push("port = ?");
        params.push(Box::new(port as i32));
    }
    if let Some(ref username) = req.username {
        sets.push("username = ?");
        params.push(Box::new(username.clone()));
    }
    if let Some(ref key_id) = req.key_id {
        sets.push("key_id = ?");
        params.push(Box::new(key_id.clone()));
    }
    if let Some(ref tags) = req.tags {
        sets.push("tags = ?");
        params.push(Box::new(
            serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string()),
        ));
    }
    if let Some(ref ai_tool_id) = req.ai_tool_id {
        sets.push("ai_tool_id = ?");
        params.push(Box::new(ai_tool_id.clone()));
    }
    if let Some(ref fingerprint) = req.host_key_fingerprint {
        sets.push("host_key_fingerprint = ?");
        params.push(Box::new(fingerprint.clone()));
    }
    if let Some(timeout) = req.idle_timeout_secs {
        sets.push("idle_timeout_secs = ?");
        params.push(Box::new(timeout as i32));
    }

    if sets.is_empty() {
        return Ok(false);
    }

    params.push(Box::new(id.to_string()));
    let sql = format!("UPDATE servers SET {} WHERE id = ?", sets.join(", "));

    let affected = conn
        .execute(
            &sql,
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        )
        .map_err(|e| e.to_string())?;

    Ok(affected > 0)
}
