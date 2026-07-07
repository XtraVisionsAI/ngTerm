use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::Database;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParamDef {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default = "default_usage")]
    pub usage: String,
}

fn default_usage() -> String {
    "env".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExternalOptions {
    #[serde(default)]
    pub detect_cmd: String,
    #[serde(default)]
    pub install_cmd: String,
    #[serde(default)]
    pub launch_cmd: String,
    #[serde(default)]
    pub config_tpl: Option<String>,
    #[serde(default)]
    pub config_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOptions {
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_force_approval")]
    pub force_approval_above: String,
    #[serde(default)]
    pub command_denylist: Vec<String>,
    #[serde(default)]
    pub risk_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub risk_overrides: HashMap<String, Vec<String>>,
}

fn default_target() -> String {
    "chat".to_string()
}

fn default_force_approval() -> String {
    "high".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AiTool {
    pub id: String,
    pub name: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub options: serde_json::Value,
    pub created_at: String,
}

impl AiTool {
    pub fn params(&self) -> Vec<ParamDef> {
        self.options
            .get("params")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn external_options(&self) -> Option<ExternalOptions> {
        self.options
            .get("external")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn execution_options(&self) -> ExecutionOptions {
        self.options
            .get("execution")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn engine_config_value(&self) -> serde_json::Value {
        self.options
            .get("engine")
            .cloned()
            .unwrap_or(serde_json::json!({}))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAiToolRequest {
    pub name: String,
    pub display_name: String,
    #[serde(default = "default_external_type")]
    pub tool_type: String,
    #[serde(default)]
    pub options: Option<serde_json::Value>,
}

fn default_external_type() -> String {
    "external".to_string()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAiToolRequest {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub tool_type: Option<String>,
    pub options: Option<serde_json::Value>,
}

pub fn create_tool(db: &Database, req: &CreateAiToolRequest) -> Result<AiTool, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let options_str =
        serde_json::to_string(&req.options.as_ref().unwrap_or(&serde_json::json!({})))
            .unwrap_or_else(|_| "{}".to_string());

    db.conn()
        .execute(
            "INSERT INTO ai_tools (id, name, display_name, type, options, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id, req.name, req.display_name, req.tool_type, options_str, now],
        )
        .map_err(|e| e.to_string())?;

    Ok(AiTool {
        id,
        name: req.name.clone(),
        display_name: req.display_name.clone(),
        tool_type: req.tool_type.clone(),
        options: req.options.clone().unwrap_or(serde_json::json!({})),
        created_at: now,
    })
}

pub fn list_tools(db: &Database) -> Result<Vec<AiTool>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT id, name, display_name, type, options, created_at FROM ai_tools ORDER BY name",
        )
        .map_err(|e| e.to_string())?;

    let tools = stmt
        .query_map([], |row| {
            let options_str: String = row.get(4)?;
            let options: serde_json::Value =
                serde_json::from_str(&options_str).unwrap_or(serde_json::json!({}));
            Ok(AiTool {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                tool_type: row.get(3)?,
                options,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(tools)
}

pub fn get_tool(db: &Database, id: &str) -> Result<Option<AiTool>, String> {
    let conn = db.conn();
    let result = conn.query_row(
        "SELECT id, name, display_name, type, options, created_at FROM ai_tools WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            let options_str: String = row.get(4)?;
            let options: serde_json::Value =
                serde_json::from_str(&options_str).unwrap_or(serde_json::json!({}));
            Ok(AiTool {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                tool_type: row.get(3)?,
                options,
                created_at: row.get(5)?,
            })
        },
    );

    match result {
        Ok(tool) => Ok(Some(tool)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn update_tool(db: &Database, id: &str, req: &UpdateAiToolRequest) -> Result<bool, String> {
    let conn = db.conn();
    let mut sets = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ref name) = req.name {
        sets.push("name = ?");
        params.push(Box::new(name.clone()));
    }
    if let Some(ref display_name) = req.display_name {
        sets.push("display_name = ?");
        params.push(Box::new(display_name.clone()));
    }
    if let Some(ref tool_type) = req.tool_type {
        sets.push("type = ?");
        params.push(Box::new(tool_type.clone()));
    }
    if let Some(ref options) = req.options {
        sets.push("options = ?");
        params.push(Box::new(
            serde_json::to_string(options).unwrap_or_else(|_| "{}".to_string()),
        ));
    }

    if sets.is_empty() {
        return Ok(false);
    }

    params.push(Box::new(id.to_string()));
    let sql = format!("UPDATE ai_tools SET {} WHERE id = ?", sets.join(", "));

    let affected = conn
        .execute(
            &sql,
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        )
        .map_err(|e| e.to_string())?;

    Ok(affected > 0)
}

pub fn delete_tool(db: &Database, id: &str) -> Result<bool, String> {
    let conn = db.conn();
    conn.execute(
        "DELETE FROM user_tool_configs WHERE tool_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM server_tool_configs WHERE tool_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;

    let affected = conn
        .execute("DELETE FROM ai_tools WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

/// Set a value at a dot-separated path in a JSON Value
pub fn set_json_path(root: &mut serde_json::Value, path: &str, value: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = root;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            if let Some(obj) = current.as_object_mut() {
                // Try to parse as number, otherwise store as string
                let json_val = if let Ok(n) = value.parse::<u64>() {
                    serde_json::Value::Number(n.into())
                } else {
                    serde_json::Value::String(value.to_string())
                };
                obj.insert(part.to_string(), json_val);
            }
        } else {
            if !current.get(*part).is_some_and(|v| v.is_object()) {
                if let Some(obj) = current.as_object_mut() {
                    obj.insert(part.to_string(), serde_json::json!({}));
                }
            }
            current = current.get_mut(*part).unwrap();
        }
    }
}
