use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::crypto;
use crate::db::Database;

pub const MASKED_VALUE: &str = "***";

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

/// Save semantics (per field):
/// * omitted (`null`) — keep the stored value;
/// * `configOverride: ""` — clear;
/// * `envValues: {}` — clear all stored values;
/// * `envValues: {k: "***"}` — keep the stored value of `k` (error if none).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveToolConfigRequest {
    pub config_override: Option<String>,
    pub env_values: Option<HashMap<String, String>>,
    pub disabled_keys: Option<Vec<String>>,
}

struct ExistingRow {
    id: String,
    config_override: Option<String>,
    disabled_keys: Vec<String>,
    has_env: bool,
    created_at: String,
}

fn load_existing(db: &Database, user_id: &str, tool_id: &str) -> Option<ExistingRow> {
    let conn = db.conn();
    conn.query_row(
        "SELECT id, config_override, disabled_keys, env_values_enc IS NOT NULL, created_at FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2",
        rusqlite::params![user_id, tool_id],
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

/// Merge masked values with the stored ones. Fails when a masked key has no
/// stored value: silently dropping it would lose the credential.
pub fn merge_masked_values(
    submitted: &HashMap<String, String>,
    existing: Option<&HashMap<String, String>>,
) -> Result<HashMap<String, String>, String> {
    let mut merged = HashMap::with_capacity(submitted.len());
    for (k, v) in submitted {
        if v == MASKED_VALUE {
            match existing.and_then(|e| e.get(k)) {
                Some(real) => {
                    merged.insert(k.clone(), real.clone());
                }
                None => {
                    return Err(format!(
                    "value for '{}' is masked but no stored value exists; submit the real value",
                    k
                ))
                }
            }
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }
    Ok(merged)
}

pub fn save_config(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    tool_id: &str,
    req: &SaveToolConfigRequest,
) -> Result<UserToolConfig, String> {
    // All reads (including decryption of the current values) happen before
    // the write lock is taken: `Database::conn()` is a non-reentrant mutex.
    let existing = load_existing(db, user_id, tool_id);
    let id = existing
        .as_ref()
        .map(|e| e.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Env values: None = keep, {} = clear, otherwise replace (masked merge).
    enum EnvAction {
        Keep,
        Clear,
        Set(Vec<u8>, Vec<u8>),
    }
    let env_action = match &req.env_values {
        None => EnvAction::Keep,
        Some(map) if map.is_empty() => EnvAction::Clear,
        Some(map) => {
            let needs_existing = map.values().any(|v| v == MASKED_VALUE);
            let current = if needs_existing {
                match existing.as_ref().map(|e| e.has_env) {
                    Some(true) => Some(get_decrypted_env(db, user_id, user_secret, tool_id)?),
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
    let keep_env = req.env_values.is_none();

    let conn = db.conn();
    conn.execute(
        "INSERT INTO user_tool_configs (id, user_id, tool_id, config_override, disabled_keys, env_values_enc, env_values_nonce, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(user_id, tool_id) DO UPDATE SET
           config_override = ?4,
           disabled_keys = ?5,
           env_values_enc = CASE WHEN ?9 THEN env_values_enc ELSE ?6 END,
           env_values_nonce = CASE WHEN ?9 THEN env_values_nonce ELSE ?7 END",
        rusqlite::params![
            id,
            user_id,
            tool_id,
            config_override,
            disabled_keys_json,
            env_enc,
            env_nonce,
            created_at,
            keep_env,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(UserToolConfig {
        id,
        user_id: user_id.to_string(),
        tool_id: tool_id.to_string(),
        config_override,
        disabled_keys,
        has_env_values: has_env,
        created_at,
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
    let row: (String, Vec<u8>, Vec<u8>) = {
        let conn = db.conn();
        conn.query_row(
            "SELECT id, env_values_enc, env_values_nonce FROM user_tool_configs WHERE user_id = ?1 AND tool_id = ?2 AND env_values_enc IS NOT NULL",
            rusqlite::params![user_id, tool_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "No env values configured for this tool".to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Database, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ngterm-utc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Database::open(dir.to_str().unwrap()).unwrap(), dir)
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Run `f` on a thread with a deadline so a re-entrant lock shows up as
    /// a failed test rather than a hung process.
    fn with_deadline<F: FnOnce() + Send + 'static>(f: F) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("operation deadlocked or panicked");
    }

    #[test]
    fn masked_save_does_not_deadlock_and_keeps_secret() {
        with_deadline(|| {
            let (db, dir) = temp_db();
            let secret = [7u8; 32];
            db.conn()
                .execute(
                    "INSERT INTO users (id, username, password, role, kdf_salt, encrypted_secret, secret_nonce, created_at) VALUES ('u1','u1','x','user',X'00',X'00',X'00','now')",
                    [],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO ai_tools (id, name, display_name, type, options, created_at) VALUES ('tool','tool','Tool','external','{}','now')",
                    [],
                )
                .unwrap();

            let first = SaveToolConfigRequest {
                config_override: Some("{\"a\":1}".into()),
                env_values: Some(map(&[("API_KEY", "real-secret"), ("MODEL", "m1")])),
                disabled_keys: Some(vec!["X".into()]),
            };
            let cfg = save_config(&db, "u1", &secret, "tool", &first).unwrap();
            assert!(cfg.has_env_values);

            // Re-save with the secret masked and a new non-secret value.
            let second = SaveToolConfigRequest {
                config_override: None,
                env_values: Some(map(&[("API_KEY", MASKED_VALUE), ("MODEL", "m2")])),
                disabled_keys: None,
            };
            let cfg = save_config(&db, "u1", &secret, "tool", &second).unwrap();
            assert!(cfg.has_env_values);
            assert_eq!(
                cfg.config_override.as_deref(),
                Some("{\"a\":1}"),
                "omitted keeps"
            );
            assert_eq!(cfg.disabled_keys, vec!["X".to_string()], "omitted keeps");
            let env = get_decrypted_env(&db, "u1", &secret, "tool").unwrap();
            assert_eq!(env["API_KEY"], "real-secret");
            assert_eq!(env["MODEL"], "m2");

            // Masked key without a stored value is an error, not a silent drop.
            let bad = SaveToolConfigRequest {
                config_override: None,
                env_values: Some(map(&[("NEW_SECRET", MASKED_VALUE)])),
                disabled_keys: None,
            };
            assert!(save_config(&db, "u1", &secret, "tool", &bad).is_err());
            let env = get_decrypted_env(&db, "u1", &secret, "tool").unwrap();
            assert_eq!(
                env["API_KEY"], "real-secret",
                "failed save left data intact"
            );

            // Explicit clear.
            let clear = SaveToolConfigRequest {
                config_override: Some(String::new()),
                env_values: Some(HashMap::new()),
                disabled_keys: Some(vec![]),
            };
            let cfg = save_config(&db, "u1", &secret, "tool", &clear).unwrap();
            assert!(!cfg.has_env_values);
            assert!(cfg.config_override.is_none());
            assert!(get_decrypted_env(&db, "u1", &secret, "tool").is_err());
            let stored = get_config(&db, "u1", "tool").unwrap().unwrap();
            assert!(!stored.has_env_values);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn merge_masked_values_rules() {
        let existing = map(&[("A", "1")]);
        let merged =
            merge_masked_values(&map(&[("A", "***"), ("B", "2")]), Some(&existing)).unwrap();
        assert_eq!(merged["A"], "1");
        assert_eq!(merged["B"], "2");
        assert!(merge_masked_values(&map(&[("C", "***")]), Some(&existing)).is_err());
        assert!(merge_masked_values(&map(&[("C", "***")]), None).is_err());
    }
}
