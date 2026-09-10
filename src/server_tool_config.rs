use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::crypto;
use crate::db::Database;
use crate::user_tool_config::{merge_masked_values, MASKED_VALUE};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerToolConfig {
    pub id: String,
    pub user_id: String,
    pub server_id: String,
    pub tool_id: String,
    pub disabled_keys: Vec<String>,
    pub config_override: Option<String>,
    pub has_env_overrides: bool,
    pub created_at: String,
}

/// Same save semantics as the user-level config: omitted keeps, empty
/// string / empty map clears, `***` keeps the stored secret.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveServerToolConfigRequest {
    pub server_id: String,
    pub env_overrides: Option<HashMap<String, String>>,
    pub disabled_keys: Option<Vec<String>>,
    pub config_override: Option<String>,
}

struct ExistingRow {
    id: String,
    config_override: Option<String>,
    disabled_keys: Vec<String>,
    has_env: bool,
    created_at: String,
}

fn load_existing(
    db: &Database,
    user_id: &str,
    server_id: &str,
    tool_id: &str,
) -> Option<ExistingRow> {
    let conn = db.conn();
    conn.query_row(
        "SELECT id, config_override, disabled_keys, env_overrides_enc IS NOT NULL, created_at FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3",
        rusqlite::params![user_id, server_id, tool_id],
        |row| {
            let disabled_keys_str: String = row
                .get::<_, Option<String>>(2)?
                .unwrap_or_else(|| "[]".to_string());
            Ok(ExistingRow {
                id: row.get(0)?,
                config_override: row.get(1)?,
                disabled_keys: serde_json::from_str(&disabled_keys_str).unwrap_or_default(),
                has_env: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )
    .ok()
}

pub fn save_config(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    tool_id: &str,
    req: &SaveServerToolConfigRequest,
) -> Result<ServerToolConfig, String> {
    // Reads (and decryption) first; the connection mutex is not re-entrant.
    let existing = load_existing(db, user_id, &req.server_id, tool_id);
    let id = existing
        .as_ref()
        .map(|e| e.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    enum EnvAction {
        Keep,
        Clear,
        Set(Vec<u8>, Vec<u8>),
    }
    let env_action = match &req.env_overrides {
        None => EnvAction::Keep,
        Some(map) if map.is_empty() => EnvAction::Clear,
        Some(map) => {
            let needs_existing = map.values().any(|v| v == MASKED_VALUE);
            let current = if needs_existing {
                match existing.as_ref().map(|e| e.has_env) {
                    Some(true) => Some(get_decrypted_env(
                        db,
                        user_id,
                        user_secret,
                        &req.server_id,
                        tool_id,
                    )?),
                    _ => None,
                }
            } else {
                None
            };
            let final_values = merge_masked_values(map, current.as_ref())?;
            let json = serde_json::to_string(&final_values).map_err(|e| e.to_string())?;
            let dek = crypto::derive_data_key(user_secret, &id);
            let (encrypted, nonce) =
                crypto::encrypt(&dek, json.as_bytes()).map_err(|e| e.to_string())?;
            EnvAction::Set(encrypted, nonce.to_vec())
        }
    };

    let config_override: Option<String> = match &req.config_override {
        None => existing.as_ref().and_then(|e| e.config_override.clone()),
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.clone()),
    };

    let disabled_keys = match &req.disabled_keys {
        Some(keys) => keys.clone(),
        None => existing
            .as_ref()
            .map(|e| e.disabled_keys.clone())
            .unwrap_or_default(),
    };
    let disabled_keys_json =
        serde_json::to_string(&disabled_keys).unwrap_or_else(|_| "[]".to_string());
    let created_at = existing
        .as_ref()
        .map(|e| e.created_at.clone())
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    let (env_enc, env_nonce, has_env): (Option<Vec<u8>>, Option<Vec<u8>>, bool) = match env_action {
        EnvAction::Set(enc, nonce) => (Some(enc), Some(nonce), true),
        EnvAction::Clear => (None, None, false),
        EnvAction::Keep => (None, None, existing.as_ref().is_some_and(|e| e.has_env)),
    };
    let keep_env = req.env_overrides.is_none();

    let conn = db.conn();
    conn.execute(
        "INSERT INTO server_tool_configs (id, user_id, server_id, tool_id, env_overrides_enc, env_overrides_nonce, disabled_keys, config_override, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(user_id, server_id, tool_id) DO UPDATE SET
           env_overrides_enc = CASE WHEN ?10 THEN env_overrides_enc ELSE ?5 END,
           env_overrides_nonce = CASE WHEN ?10 THEN env_overrides_nonce ELSE ?6 END,
           disabled_keys = ?7,
           config_override = ?8",
        rusqlite::params![
            id,
            user_id,
            req.server_id,
            tool_id,
            env_enc,
            env_nonce,
            disabled_keys_json,
            config_override,
            created_at,
            keep_env,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(ServerToolConfig {
        id,
        user_id: user_id.to_string(),
        server_id: req.server_id.clone(),
        tool_id: tool_id.to_string(),
        disabled_keys,
        config_override,
        has_env_overrides: has_env,
        created_at,
    })
}

pub fn get_config(
    db: &Database,
    user_id: &str,
    server_id: &str,
    tool_id: &str,
) -> Result<Option<ServerToolConfig>, String> {
    let conn = db.conn();
    let result = conn.query_row(
        "SELECT id, user_id, server_id, tool_id, disabled_keys, config_override, env_overrides_enc IS NOT NULL, created_at FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3",
        rusqlite::params![user_id, server_id, tool_id],
        |row| {
            let disabled_keys_str: String = row.get(4)?;
            let disabled_keys: Vec<String> = serde_json::from_str(&disabled_keys_str).unwrap_or_default();
            Ok(ServerToolConfig {
                id: row.get(0)?,
                user_id: row.get(1)?,
                server_id: row.get(2)?,
                tool_id: row.get(3)?,
                disabled_keys,
                config_override: row.get(5)?,
                has_env_overrides: row.get(6)?,
                created_at: row.get(7)?,
            })
        },
    );

    match result {
        Ok(cfg) => Ok(Some(cfg)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn get_decrypted_env(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    server_id: &str,
    tool_id: &str,
) -> Result<HashMap<String, String>, String> {
    let row: (String, Vec<u8>, Vec<u8>) = {
        let conn = db.conn();
        conn.query_row(
            "SELECT id, env_overrides_enc, env_overrides_nonce FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3 AND env_overrides_enc IS NOT NULL",
            rusqlite::params![user_id, server_id, tool_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "No server env overrides configured".to_string())?
    };

    let (id, encrypted, nonce_vec) = row;
    let nonce: [u8; 12] = nonce_vec
        .try_into()
        .map_err(|_| "Invalid nonce".to_string())?;

    let dek = crypto::derive_data_key(user_secret, &id);
    let plaintext = crypto::decrypt(&dek, &nonce, &encrypted).map_err(|_| {
        "Stored values cannot be decrypted with the current credentials".to_string()
    })?;
    let json_str = String::from_utf8(plaintext).map_err(|_| "Invalid env data".to_string())?;

    serde_json::from_str(&json_str).map_err(|e| e.to_string())
}

pub fn delete_config(
    db: &Database,
    user_id: &str,
    server_id: &str,
    tool_id: &str,
) -> Result<bool, String> {
    let conn = db.conn();
    let affected = conn
        .execute(
            "DELETE FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3",
            rusqlite::params![user_id, server_id, tool_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_server_save_completes_and_clears_correctly() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = std::env::temp_dir().join(format!("ngterm-stc-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = Database::open(dir.to_str().unwrap()).unwrap();
            let secret = [9u8; 32];
            let mk = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            };

            let r = SaveServerToolConfigRequest {
                server_id: "s1".into(),
                env_overrides: Some(mk(&[("KEY", "v1")])),
                disabled_keys: None,
                config_override: Some("{\"approvalLevel\":\"all\"}".into()),
            };
            let cfg = save_config(&db, "u", &secret, "t", &r).unwrap();
            assert!(cfg.has_env_overrides);

            let r = SaveServerToolConfigRequest {
                server_id: "s1".into(),
                env_overrides: Some(mk(&[("KEY", "***")])),
                disabled_keys: None,
                config_override: None,
            };
            let cfg = save_config(&db, "u", &secret, "t", &r).unwrap();
            assert_eq!(
                cfg.config_override.as_deref(),
                Some("{\"approvalLevel\":\"all\"}")
            );
            assert_eq!(
                get_decrypted_env(&db, "u", &secret, "s1", "t").unwrap()["KEY"],
                "v1"
            );

            let r = SaveServerToolConfigRequest {
                server_id: "s1".into(),
                env_overrides: Some(HashMap::new()),
                disabled_keys: None,
                config_override: None,
            };
            let cfg = save_config(&db, "u", &secret, "t", &r).unwrap();
            assert!(!cfg.has_env_overrides);
            assert!(get_decrypted_env(&db, "u", &secret, "s1", "t").is_err());
            let _ = std::fs::remove_dir_all(&dir);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("server config save deadlocked");
    }
}
