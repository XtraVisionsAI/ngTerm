use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::crypto;
use crate::db::Database;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserToolConfig {
    pub id: String,
    pub user_id: String,
    pub tool_id: String,
    pub config_override: Option<String>,
    pub disabled_keys: Vec<String>,
    pub has_env_values: bool,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveToolConfigRequest {
    pub config_override: Option<String>,
    pub env_values: Option<HashMap<String, String>>,
    pub disabled_keys: Option<Vec<String>>,
}

pub fn save_config(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    tool_id: &str,
    req: &SaveToolConfigRequest,
) -> Result<UserToolConfig, String> {
    let conn = db.conn();

    let existing_id: Option<String> = conn
        .query_row(
            "SELECT id FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2",
            rusqlite::params![user_id, tool_id],
            |row| row.get(0),
        )
        .ok();

    let id = existing_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let (env_enc, env_nonce): (Option<Vec<u8>>, Option<Vec<u8>>) =
        if let Some(ref env_values) = req.env_values {
            // Merge masked values ("***") with existing stored values
            let mut final_values = env_values.clone();
            if final_values.values().any(|v| v == "***") {
                if let Ok(existing) = get_decrypted_env(db, user_id, user_secret, tool_id) {
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
        } else {
            (None, None)
        };

    let now = Utc::now().to_rfc3339();
    let disabled_keys = req.disabled_keys.clone().unwrap_or_default();
    let disabled_keys_json =
        serde_json::to_string(&disabled_keys).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO user_tool_configs (id, user_id, tool_id, config_override, disabled_keys, env_values_enc, env_values_nonce, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, tool_id) DO UPDATE SET
           config_override = COALESCE(?4, config_override),
           disabled_keys = ?5,
           env_values_enc = COALESCE(?6, env_values_enc),
           env_values_nonce = COALESCE(?7, env_values_nonce)",
        rusqlite::params![
            id,
            user_id,
            tool_id,
            req.config_override,
            disabled_keys_json,
            env_enc,
            env_nonce,
            now
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(UserToolConfig {
        id,
        user_id: user_id.to_string(),
        tool_id: tool_id.to_string(),
        config_override: req.config_override.clone(),
        disabled_keys,
        has_env_values: req.env_values.is_some() || env_enc.is_some(),
        created_at: now,
    })
}

pub fn get_config(
    db: &Database,
    user_id: &str,
    tool_id: &str,
) -> Result<Option<UserToolConfig>, String> {
    let conn = db.conn();
    let result = conn.query_row(
        "SELECT id, user_id, tool_id, config_override, disabled_keys, env_values_enc IS NOT NULL, created_at FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2",
        rusqlite::params![user_id, tool_id],
        |row| {
            let disabled_keys_str: String = row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "[]".to_string());
            let disabled_keys: Vec<String> = serde_json::from_str(&disabled_keys_str).unwrap_or_default();
            Ok(UserToolConfig {
                id: row.get(0)?,
                user_id: row.get(1)?,
                tool_id: row.get(2)?,
                config_override: row.get(3)?,
                disabled_keys,
                has_env_values: row.get(5)?,
                created_at: row.get(6)?,
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
    tool_id: &str,
) -> Result<HashMap<String, String>, String> {
    let conn = db.conn();
    let row: (String, Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT id, env_values_enc, env_values_nonce FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2 AND env_values_enc IS NOT NULL",
            rusqlite::params![user_id, tool_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "No env values configured for this tool".to_string())?;

    let (id, encrypted, nonce_vec) = row;
    let nonce: [u8; 12] = nonce_vec
        .try_into()
        .map_err(|_| "Invalid nonce".to_string())?;

    let dek = crypto::derive_data_key(user_secret, &id);
    let plaintext = crypto::decrypt(&dek, &nonce, &encrypted).map_err(|e| e.to_string())?;
    let json_str = String::from_utf8(plaintext).map_err(|_| "Invalid env data".to_string())?;

    serde_json::from_str(&json_str).map_err(|e| e.to_string())
}

pub fn list_configs(db: &Database, user_id: &str) -> Result<Vec<UserToolConfig>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, tool_id, config_override, disabled_keys, env_values_enc IS NOT NULL, created_at FROM user_tool_configs WHERE user_id = ?1",
        )
        .map_err(|e| e.to_string())?;

    let configs = stmt
        .query_map(rusqlite::params![user_id], |row| {
            let disabled_keys_str: String = row
                .get::<_, Option<String>>(4)?
                .unwrap_or_else(|| "[]".to_string());
            let disabled_keys: Vec<String> =
                serde_json::from_str(&disabled_keys_str).unwrap_or_default();
            Ok(UserToolConfig {
                id: row.get(0)?,
                user_id: row.get(1)?,
                tool_id: row.get(2)?,
                config_override: row.get(3)?,
                disabled_keys,
                has_env_values: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(configs)
}

pub fn delete_config(db: &Database, user_id: &str, tool_id: &str) -> Result<bool, String> {
    let conn = db.conn();
    let affected = conn
        .execute(
            "DELETE FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2",
            rusqlite::params![user_id, tool_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}
