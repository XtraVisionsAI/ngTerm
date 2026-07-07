use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::crypto;
use crate::db::Database;

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveServerToolConfigRequest {
    pub server_id: String,
    pub env_overrides: Option<HashMap<String, String>>,
    pub disabled_keys: Option<Vec<String>>,
    pub config_override: Option<String>,
}

pub fn save_config(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    tool_id: &str,
    req: &SaveServerToolConfigRequest,
) -> Result<ServerToolConfig, String> {
    let conn = db.conn();

    let existing_id: Option<String> = conn
        .query_row(
            "SELECT id FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3",
            rusqlite::params![user_id, req.server_id, tool_id],
            |row| row.get(0),
        )
        .ok();

    let id = existing_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let (env_enc, env_nonce): (Option<Vec<u8>>, Option<Vec<u8>>) =
        if let Some(ref env_overrides) = req.env_overrides {
            if env_overrides.is_empty() {
                (None, None)
            } else {
                let mut final_values = env_overrides.clone();
                if final_values.values().any(|v| v == "***") {
                    if let Ok(existing) =
                        get_decrypted_env(db, user_id, user_secret, &req.server_id, tool_id)
                    {
                        for (k, v) in &mut final_values {
                            if v == "***" {
                                if let Some(real) = existing.get(k) {
                                    *v = real.clone();
                                }
                            }
                        }
                    }
                    final_values.retain(|_, v| v != "***");
                }
                let json = serde_json::to_string(&final_values).map_err(|e| e.to_string())?;
                let dek = crypto::derive_data_key(user_secret, &id);
                let (encrypted, nonce) =
                    crypto::encrypt(&dek, json.as_bytes()).map_err(|e| e.to_string())?;
                (Some(encrypted), Some(nonce.to_vec()))
            }
        } else {
            (None, None)
        };

    let disabled_keys = req.disabled_keys.clone().unwrap_or_default();
    let disabled_keys_json =
        serde_json::to_string(&disabled_keys).unwrap_or_else(|_| "[]".to_string());
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO server_tool_configs (id, user_id, server_id, tool_id, env_overrides_enc, env_overrides_nonce, disabled_keys, config_override, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(user_id, server_id, tool_id) DO UPDATE SET
           env_overrides_enc = COALESCE(?5, env_overrides_enc),
           env_overrides_nonce = COALESCE(?6, env_overrides_nonce),
           disabled_keys = ?7,
           config_override = COALESCE(?8, config_override)",
        rusqlite::params![
            id,
            user_id,
            req.server_id,
            tool_id,
            env_enc,
            env_nonce,
            disabled_keys_json,
            req.config_override,
            now
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(ServerToolConfig {
        id,
        user_id: user_id.to_string(),
        server_id: req.server_id.clone(),
        tool_id: tool_id.to_string(),
        disabled_keys,
        config_override: req.config_override.clone(),
        has_env_overrides: req.env_overrides.is_some(),
        created_at: now,
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
    let conn = db.conn();
    let row: (String, Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT id, env_overrides_enc, env_overrides_nonce FROM server_tool_configs WHERE user_id = ?1 AND server_id = ?2 AND tool_id = ?3 AND env_overrides_enc IS NOT NULL",
            rusqlite::params![user_id, server_id, tool_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "No server env overrides configured".to_string())?;

    let (id, encrypted, nonce_vec) = row;
    let nonce: [u8; 12] = nonce_vec
        .try_into()
        .map_err(|_| "Invalid nonce".to_string())?;

    let dek = crypto::derive_data_key(user_secret, &id);
    let plaintext = crypto::decrypt(&dek, &nonce, &encrypted).map_err(|e| e.to_string())?;
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
