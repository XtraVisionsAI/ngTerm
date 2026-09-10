use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::db::Database;

/// Current version of the `options` document stored for every AI tool.
/// Version 0 (implicit) allowed mixed camelCase/snake_case keys inside
/// `options.external` / `options.execution`; version 1 is snake_case only.
pub const OPTIONS_VERSION: u64 = 1;

pub const RISK_LEVELS: &[&str] = &["low", "medium", "high", "critical"];
pub const EXECUTION_TARGETS: &[&str] = &["chat", "terminal", "both"];
pub const PARAM_USAGES: &[&str] = &["env", "config"];
pub const TOOL_TYPES: &[&str] = &["external", "native"];

/// A field-level validation error returned to API callers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub enum RegistryError {
    Validation(Vec<FieldError>),
    NotFound,
    Db(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Validation(errs) => {
                let msgs: Vec<String> = errs
                    .iter()
                    .map(|e| format!("{}: {}", e.field, e.message))
                    .collect();
                write!(f, "invalid tool options: {}", msgs.join("; "))
            }
            RegistryError::NotFound => write!(f, "tool not found"),
            RegistryError::Db(e) => write!(f, "{}", e),
        }
    }
}

type EngineValidator = Box<dyn Fn(&serde_json::Value) -> Result<(), Vec<FieldError>> + Send + Sync>;

static ENGINE_VALIDATOR: OnceLock<EngineValidator> = OnceLock::new();

/// Downstream distributions that implement `native` tools register a
/// validator for `options.engine` here. Without one, native tools are
/// rejected at save time because nothing could run them.
pub fn register_engine_validator(validator: EngineValidator) {
    let _ = ENGINE_VALIDATOR.set(validator);
}

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

/// Options for tools that launch an external CLI agent.
///
/// Stored in snake_case (matching the admin form and seed data). The
/// camelCase aliases accept documents written before `OPTIONS_VERSION` 1;
/// a document containing both spellings of a field is rejected by serde as
/// a duplicate field.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ExternalOptions {
    #[serde(default, alias = "detectCmd")]
    pub detect_cmd: String,
    #[serde(default, alias = "installCmd")]
    pub install_cmd: String,
    #[serde(default, alias = "launchCmd")]
    pub launch_cmd: String,
    #[serde(default, alias = "configTpl")]
    pub config_tpl: Option<String>,
    #[serde(default, alias = "configPath")]
    pub config_path: Option<String>,
    /// Whether the CLI can route per-operation approvals through the
    /// platform. External CLIs generally cannot; the UI must not promise
    /// per-item approval for them.
    #[serde(default, alias = "supportsApproval")]
    pub supports_approval: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecutionOptions {
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default = "default_force_approval", alias = "forceApprovalAbove")]
    pub force_approval_above: String,
    #[serde(default, alias = "commandDenylist")]
    pub command_denylist: Vec<String>,
    #[serde(default, alias = "riskRules")]
    pub risk_rules: Option<serde_json::Value>,
    #[serde(default, alias = "riskOverrides")]
    pub risk_overrides: HashMap<String, Vec<String>>,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            target: default_target(),
            force_approval_above: default_force_approval(),
            command_denylist: Vec::new(),
            risk_rules: None,
            risk_overrides: HashMap::new(),
        }
    }
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

    /// Parsed `options.external`. Missing section yields defaults; a
    /// malformed section is an error (never silently empty launch commands).
    pub fn external_options(&self) -> Result<ExternalOptions, String> {
        match self.options.get("external") {
            None | Some(serde_json::Value::Null) => Ok(ExternalOptions::default()),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| format!("invalid options.external: {}", e)),
        }
    }

    /// Parsed `options.execution`. Missing section yields defaults; a
    /// malformed section is an error so policy is never silently dropped.
    pub fn execution_options(&self) -> Result<ExecutionOptions, String> {
        match self.options.get("execution") {
            None | Some(serde_json::Value::Null) => Ok(ExecutionOptions::default()),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| format!("invalid options.execution: {}", e)),
        }
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

// --- Normalisation & validation ---

const EXTERNAL_KEYS: &[(&str, &str)] = &[
    ("detectCmd", "detect_cmd"),
    ("installCmd", "install_cmd"),
    ("launchCmd", "launch_cmd"),
    ("configTpl", "config_tpl"),
    ("configPath", "config_path"),
    ("supportsApproval", "supports_approval"),
];

const EXECUTION_KEYS: &[(&str, &str)] = &[
    ("forceApprovalAbove", "force_approval_above"),
    ("commandDenylist", "command_denylist"),
    ("riskRules", "risk_rules"),
    ("riskOverrides", "risk_overrides"),
];

fn normalize_section(
    options: &mut serde_json::Value,
    section: &str,
    keys: &[(&str, &str)],
    errors: &mut Vec<FieldError>,
) {
    let Some(obj) = options.get_mut(section).and_then(|v| v.as_object_mut()) else {
        return;
    };
    for (camel, snake) in keys {
        if let Some(legacy) = obj.remove(*camel) {
            match obj.get(*snake) {
                None => {
                    obj.insert(snake.to_string(), legacy);
                }
                Some(current) if *current == legacy => {}
                Some(_) => {
                    errors.push(FieldError::new(
                        format!("{}.{}", section, snake),
                        format!(
                            "conflicting values for '{}' and legacy alias '{}'",
                            snake, camel
                        ),
                    ));
                    // Keep the legacy value visible so serde also rejects it.
                    obj.insert(camel.to_string(), legacy);
                }
            }
        }
    }
}

/// Rewrite legacy camelCase keys inside `external` / `execution` to
/// snake_case and stamp `options_version`. Returns field errors when the
/// same field is present in both spellings with different values.
pub fn normalize_options(options: &mut serde_json::Value) -> Result<bool, Vec<FieldError>> {
    if options.is_null() {
        *options = serde_json::json!({});
    }
    let before = options.clone();
    let mut errors = Vec::new();

    if !options.is_object() {
        return Err(vec![FieldError::new("options", "must be a JSON object")]);
    }

    normalize_section(options, "external", EXTERNAL_KEYS, &mut errors);
    normalize_section(options, "execution", EXECUTION_KEYS, &mut errors);

    if let Some(obj) = options.as_object_mut() {
        obj.insert(
            "options_version".to_string(),
            serde_json::Value::Number(OPTIONS_VERSION.into()),
        );
    }

    if errors.is_empty() {
        Ok(*options != before)
    } else {
        Err(errors)
    }
}

fn check_regex_list(prefix: &str, list: &[String], errors: &mut Vec<FieldError>) {
    for (i, p) in list.iter().enumerate() {
        if let Err(e) = regex_lite_check(p) {
            errors.push(FieldError::new(format!("{}[{}]", prefix, i), e));
        }
    }
}

/// Compile check for policy regexes. Uses the same syntax family as the
/// engine (Rust `regex`); an invalid pattern must be rejected at save time
/// rather than dropped at compile time.
fn regex_lite_check(pattern: &str) -> Result<(), String> {
    regex::Regex::new(pattern)
        .map(|_| ())
        .map_err(|e| format!("invalid regex '{}': {}", pattern, e))
}

/// Validate a tool's `options` document (after normalisation).
pub fn validate_options(
    tool_type: &str,
    options: &serde_json::Value,
) -> Result<(), Vec<FieldError>> {
    let mut errors = Vec::new();

    if !TOOL_TYPES.contains(&tool_type) {
        errors.push(FieldError::new(
            "type",
            format!(
                "unknown tool type '{}', expected external or native",
                tool_type
            ),
        ));
    }

    let Some(obj) = options.as_object() else {
        return Err(vec![FieldError::new("options", "must be a JSON object")]);
    };

    // params
    if let Some(params_val) = obj.get("params") {
        match serde_json::from_value::<Vec<ParamDef>>(params_val.clone()) {
            Ok(params) => {
                let mut seen = std::collections::HashSet::new();
                for (i, p) in params.iter().enumerate() {
                    if p.key.trim().is_empty() {
                        errors.push(FieldError::new(
                            format!("params[{}].key", i),
                            "key must not be empty",
                        ));
                    } else if !seen.insert(p.key.clone()) {
                        errors.push(FieldError::new(
                            format!("params[{}].key", i),
                            format!("duplicate param key '{}'", p.key),
                        ));
                    }
                    if !PARAM_USAGES.contains(&p.usage.as_str()) {
                        errors.push(FieldError::new(
                            format!("params[{}].usage", i),
                            format!("unknown usage '{}', expected env or config", p.usage),
                        ));
                    }
                }
            }
            Err(e) => errors.push(FieldError::new("params", e.to_string())),
        }
    }

    // external
    match obj.get("external") {
        None | Some(serde_json::Value::Null) => {
            if tool_type == "external" {
                errors.push(FieldError::new(
                    "external.launch_cmd",
                    "external tools require a launch command",
                ));
            }
        }
        Some(v) => match serde_json::from_value::<ExternalOptions>(v.clone()) {
            Ok(ext) => {
                if tool_type == "external" && ext.launch_cmd.trim().is_empty() {
                    errors.push(FieldError::new(
                        "external.launch_cmd",
                        "launch command must not be empty",
                    ));
                }
                if let Some(tpl) = &ext.config_tpl {
                    if !tpl.trim().is_empty() {
                        if let Err(e) = serde_json::from_str::<serde_json::Value>(tpl) {
                            errors.push(FieldError::new(
                                "external.config_tpl",
                                format!("must be valid JSON: {}", e),
                            ));
                        }
                    }
                }
            }
            Err(e) => errors.push(FieldError::new("external", e.to_string())),
        },
    }

    // execution
    if let Some(v) = obj.get("execution") {
        if !v.is_null() {
            match serde_json::from_value::<ExecutionOptions>(v.clone()) {
                Ok(exec) => {
                    if !EXECUTION_TARGETS.contains(&exec.target.as_str()) {
                        errors.push(FieldError::new(
                            "execution.target",
                            format!(
                                "unknown target '{}', expected chat, terminal or both",
                                exec.target
                            ),
                        ));
                    }
                    if !RISK_LEVELS.contains(&exec.force_approval_above.as_str()) {
                        errors.push(FieldError::new(
                            "execution.force_approval_above",
                            format!(
                                "unknown risk level '{}', expected low, medium, high or critical",
                                exec.force_approval_above
                            ),
                        ));
                    }
                    check_regex_list(
                        "execution.command_denylist",
                        &exec.command_denylist,
                        &mut errors,
                    );
                    for (level, patterns) in &exec.risk_overrides {
                        if !RISK_LEVELS.contains(&level.as_str()) {
                            errors.push(FieldError::new(
                                format!("execution.risk_overrides.{}", level),
                                format!("unknown risk level '{}'", level),
                            ));
                        }
                        check_regex_list(
                            &format!("execution.risk_overrides.{}", level),
                            patterns,
                            &mut errors,
                        );
                    }
                    if let Some(rules) = &exec.risk_rules {
                        match rules.as_object() {
                            Some(map) => {
                                for (level, patterns) in map {
                                    if !RISK_LEVELS.contains(&level.as_str()) {
                                        errors.push(FieldError::new(
                                            format!("execution.risk_rules.{}", level),
                                            format!("unknown risk level '{}'", level),
                                        ));
                                    }
                                    match serde_json::from_value::<Vec<String>>(patterns.clone()) {
                                        Ok(list) => check_regex_list(
                                            &format!("execution.risk_rules.{}", level),
                                            &list,
                                            &mut errors,
                                        ),
                                        Err(_) => errors.push(FieldError::new(
                                            format!("execution.risk_rules.{}", level),
                                            "must be a list of regex strings",
                                        )),
                                    }
                                }
                            }
                            None => errors.push(FieldError::new(
                                "execution.risk_rules",
                                "must be an object keyed by risk level",
                            )),
                        }
                    }
                }
                Err(e) => errors.push(FieldError::new("execution", e.to_string())),
            }
        }
    }

    // engine (native only)
    if tool_type == "native" {
        let engine = obj.get("engine").cloned().unwrap_or(serde_json::json!({}));
        match ENGINE_VALIDATOR.get() {
            Some(validator) => {
                if let Err(mut errs) = validator(&engine) {
                    for e in &mut errs {
                        e.field = format!("engine.{}", e.field);
                    }
                    errors.extend(errs);
                }
            }
            None => errors.push(FieldError::new(
                "type",
                "native engine tools are not supported by this edition",
            )),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Normalise then validate, returning the canonical document.
pub fn prepare_options(
    tool_type: &str,
    options: Option<&serde_json::Value>,
) -> Result<serde_json::Value, RegistryError> {
    let mut opts = options.cloned().unwrap_or(serde_json::json!({}));
    normalize_options(&mut opts).map_err(RegistryError::Validation)?;
    validate_options(tool_type, &opts).map_err(RegistryError::Validation)?;
    Ok(opts)
}

// --- CRUD ---

pub fn create_tool(db: &Database, req: &CreateAiToolRequest) -> Result<AiTool, RegistryError> {
    if req.name.trim().is_empty() {
        return Err(RegistryError::Validation(vec![FieldError::new(
            "name",
            "name must not be empty",
        )]));
    }
    let options = prepare_options(&req.tool_type, req.options.as_ref())?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let options_str = serde_json::to_string(&options).unwrap_or_else(|_| "{}".to_string());

    db.conn()
        .execute(
            "INSERT INTO ai_tools (id, name, display_name, type, options, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id, req.name, req.display_name, req.tool_type, options_str, now],
        )
        .map_err(|e| RegistryError::Db(e.to_string()))?;

    Ok(AiTool {
        id,
        name: req.name.clone(),
        display_name: req.display_name.clone(),
        tool_type: req.tool_type.clone(),
        options,
        created_at: now,
    })
}

fn row_to_tool(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiTool> {
    let options_str: String = row.get(4)?;
    let mut options: serde_json::Value =
        serde_json::from_str(&options_str).unwrap_or(serde_json::json!({}));
    // In-memory normalisation for rows written before the migration ran.
    let _ = normalize_options(&mut options);
    Ok(AiTool {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        tool_type: row.get(3)?,
        options,
        created_at: row.get(5)?,
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
        .query_map([], row_to_tool)
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
        row_to_tool,
    );

    match result {
        Ok(tool) => Ok(Some(tool)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn update_tool(
    db: &Database,
    id: &str,
    req: &UpdateAiToolRequest,
) -> Result<bool, RegistryError> {
    let existing = get_tool(db, id)
        .map_err(RegistryError::Db)?
        .ok_or(RegistryError::NotFound)?;

    let effective_type = req
        .tool_type
        .clone()
        .unwrap_or_else(|| existing.tool_type.clone());

    let prepared_options = match &req.options {
        Some(opts) => Some(prepare_options(&effective_type, Some(opts))?),
        None if req.tool_type.is_some() => {
            // Type changed without new options: the stored options must still
            // be valid for the new type.
            validate_options(&effective_type, &existing.options)
                .map_err(RegistryError::Validation)?;
            None
        }
        None => None,
    };

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
    if let Some(ref options) = prepared_options {
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
        .map_err(|e| RegistryError::Db(e.to_string()))?;

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

/// Startup migration: rewrite every stored `options` document into the
/// canonical (snake_case, versioned) form. Rows whose legacy and canonical
/// keys conflict are left untouched and logged; they surface as validation
/// errors when an administrator next edits them.
pub fn migrate_tool_options_conn(conn: &rusqlite::Connection) -> Result<usize, String> {
    let mut stmt = conn
        .prepare("SELECT id, options FROM ai_tools")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut updated = 0;
    for (id, options_str) in rows {
        let mut options: serde_json::Value = match serde_json::from_str(&options_str) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("ai_tools[{}] has unparsable options, skipping: {}", id, e);
                continue;
            }
        };
        match normalize_options(&mut options) {
            Ok(true) => {
                let s = serde_json::to_string(&options).unwrap_or_else(|_| "{}".to_string());
                conn.execute(
                    "UPDATE ai_tools SET options = ?1 WHERE id = ?2",
                    rusqlite::params![s, id],
                )
                .map_err(|e| e.to_string())?;
                updated += 1;
            }
            Ok(false) => {}
            Err(errs) => {
                tracing::warn!(
                    "ai_tools[{}] options could not be migrated automatically: {:?}",
                    id,
                    errs
                );
            }
        }
    }
    Ok(updated)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_with(options: serde_json::Value) -> AiTool {
        AiTool {
            id: "t".into(),
            name: "t".into(),
            display_name: "T".into(),
            tool_type: "external".into(),
            options,
            created_at: String::new(),
        }
    }

    #[test]
    fn snake_case_seed_data_is_honoured() {
        let t = tool_with(json!({
            "external": {"detect_cmd": "command -v claude", "launch_cmd": "claude -p"},
            "execution": {
                "target": "both",
                "force_approval_above": "medium",
                "command_denylist": ["rm\\s+-rf\\s+/"],
                "risk_overrides": {"low": ["^ls\\b"]}
            }
        }));
        let ext = t.external_options().unwrap();
        assert_eq!(ext.launch_cmd, "claude -p");
        let exec = t.execution_options().unwrap();
        assert_eq!(exec.target, "both");
        assert_eq!(exec.force_approval_above, "medium");
        assert_eq!(exec.command_denylist.len(), 1);
        assert_eq!(exec.risk_overrides["low"].len(), 1);
    }

    #[test]
    fn legacy_camel_case_is_accepted_and_normalised() {
        let mut opts = json!({
            "external": {"launchCmd": "claude", "detectCmd": "which claude"},
            "execution": {"forceApprovalAbove": "critical", "commandDenylist": ["x"]}
        });
        let changed = normalize_options(&mut opts).unwrap();
        assert!(changed);
        assert_eq!(opts["external"]["launch_cmd"], "claude");
        assert!(opts["external"].get("launchCmd").is_none());
        assert_eq!(opts["execution"]["force_approval_above"], "critical");
        assert_eq!(opts["options_version"], OPTIONS_VERSION);

        // Second pass is a no-op.
        assert!(!normalize_options(&mut opts).unwrap());

        // Parsing also accepts aliases directly.
        let t = tool_with(json!({"execution": {"forceApprovalAbove": "low"}}));
        assert_eq!(t.execution_options().unwrap().force_approval_above, "low");
    }

    #[test]
    fn conflicting_aliases_are_rejected() {
        let mut opts = json!({
            "execution": {"forceApprovalAbove": "low", "force_approval_above": "high"}
        });
        let errs = normalize_options(&mut opts).unwrap_err();
        assert_eq!(errs[0].field, "execution.force_approval_above");

        let t = tool_with(opts);
        assert!(t.execution_options().is_err());
    }

    #[test]
    fn validation_reports_field_level_errors() {
        let opts = json!({
            "params": [{"key": "A", "usage": "magic"}, {"key": "A"}, {"key": ""}],
            "external": {"launch_cmd": "", "config_tpl": "{not json"},
            "execution": {
                "target": "everywhere",
                "force_approval_above": "extreme",
                "command_denylist": ["(bad"],
                "risk_overrides": {"nope": ["^x"], "low": ["(bad"]},
                "risk_rules": {"high": "not-a-list"}
            }
        });
        let errs = validate_options("external", &opts).unwrap_err();
        let fields: Vec<&str> = errs.iter().map(|e| e.field.as_str()).collect();
        for expected in [
            "params[0].usage",
            "params[1].key",
            "params[2].key",
            "external.launch_cmd",
            "external.config_tpl",
            "execution.target",
            "execution.force_approval_above",
            "execution.command_denylist[0]",
            "execution.risk_overrides.nope",
            "execution.risk_overrides.low[0]",
            "execution.risk_rules.high",
        ] {
            assert!(
                fields.contains(&expected),
                "missing {} in {:?}",
                expected,
                fields
            );
        }
    }

    #[test]
    fn native_without_engine_validator_is_rejected() {
        // No validator registered in this test binary.
        let errs = validate_options("native", &json!({})).unwrap_err();
        assert_eq!(errs[0].field, "type");
    }

    #[test]
    fn valid_external_options_pass() {
        let opts = json!({
            "params": [{"key": "ANTHROPIC_API_KEY", "secret": true, "usage": "env"}],
            "external": {"launch_cmd": "claude -p", "config_tpl": "{}"},
            "execution": {"target": "chat", "force_approval_above": "high"}
        });
        assert!(validate_options("external", &opts).is_ok());
    }

    #[test]
    fn crud_normalises_and_migrates_stored_rows() {
        let dir = std::env::temp_dir().join(format!("ngterm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(dir.to_str().unwrap()).unwrap();

        // Insert a legacy row directly, bypassing validation.
        db.conn()
            .execute(
                "INSERT INTO ai_tools (id, name, display_name, type, options, created_at) VALUES ('legacy','legacy','L','external',?1,'now')",
                rusqlite::params![json!({"external": {"launchCmd": "old-cli"}}).to_string()],
            )
            .unwrap();
        let updated = migrate_tool_options_conn(&db.conn()).unwrap();
        assert!(updated >= 1);
        let t = get_tool(&db, "legacy").unwrap().unwrap();
        assert_eq!(t.external_options().unwrap().launch_cmd, "old-cli");
        assert_eq!(t.options["options_version"], OPTIONS_VERSION);

        // Create rejects invalid, accepts valid and keeps policy fields.
        let bad = CreateAiToolRequest {
            name: "bad".into(),
            display_name: "Bad".into(),
            tool_type: "external".into(),
            options: Some(
                json!({"external": {"launch_cmd": "x"}, "execution": {"target": "nowhere"}}),
            ),
        };
        assert!(matches!(
            create_tool(&db, &bad),
            Err(RegistryError::Validation(_))
        ));

        let good = CreateAiToolRequest {
            name: "good".into(),
            display_name: "Good".into(),
            tool_type: "external".into(),
            options: Some(json!({
                "external": {"launchCmd": "x"},
                "execution": {"target": "chat", "command_denylist": ["^rm"], "risk_overrides": {"low": ["^ls"]}}
            })),
        };
        let created = create_tool(&db, &good).unwrap();
        let stored = get_tool(&db, &created.id).unwrap().unwrap();
        let exec = stored.execution_options().unwrap();
        assert_eq!(exec.command_denylist, vec!["^rm".to_string()]);
        assert_eq!(exec.risk_overrides["low"], vec!["^ls".to_string()]);
        assert_eq!(stored.external_options().unwrap().launch_cmd, "x");

        // Update with invalid options is refused and leaves the row intact.
        let upd = UpdateAiToolRequest {
            name: None,
            display_name: None,
            tool_type: None,
            options: Some(
                json!({"external": {"launch_cmd": "x"}, "execution": {"command_denylist": ["(bad"]}}),
            ),
        };
        assert!(matches!(
            update_tool(&db, &created.id, &upd),
            Err(RegistryError::Validation(_))
        ));
        let stored = get_tool(&db, &created.id).unwrap().unwrap();
        assert_eq!(
            stored.execution_options().unwrap().command_denylist.len(),
            1
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
