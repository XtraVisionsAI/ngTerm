use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::db::Database;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub key_type: String,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeyRequest {
    pub name: String,
    pub private_key: String,
}

pub fn create_key(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    name: &str,
    private_key_pem: &str,
) -> Result<KeyInfo, String> {
    let key_id = uuid::Uuid::new_v4().to_string();

    let key_type = detect_key_type(private_key_pem);
    let fingerprint = compute_fingerprint(private_key_pem);

    let dek = crypto::derive_data_key(user_secret, &key_id);
    let (encrypted, nonce) =
        crypto::encrypt(&dek, private_key_pem.as_bytes()).map_err(|e| e.to_string())?;

    let now = Utc::now().to_rfc3339();

    let conn = db.conn();
    conn.execute(
        "INSERT INTO keys (id, user_id, name, fingerprint, key_type, encrypted, nonce, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![key_id, user_id, name, fingerprint, key_type, encrypted, nonce.as_slice(), now],
    ).map_err(|e| e.to_string())?;

    Ok(KeyInfo {
        id: key_id,
        name: name.to_string(),
        fingerprint,
        key_type,
        created_at: now,
    })
}

pub fn list_keys(db: &Database, user_id: &str) -> Result<Vec<KeyInfo>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, name, fingerprint, key_type, created_at FROM keys WHERE user_id = ?1")
        .map_err(|e| e.to_string())?;

    let keys = stmt
        .query_map(rusqlite::params![user_id], |row| {
            Ok(KeyInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                fingerprint: row.get(2)?,
                key_type: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(keys)
}

pub fn delete_key(db: &Database, user_id: &str, key_id: &str) -> Result<bool, String> {
    let conn = db.conn();
    conn.execute(
        "UPDATE servers SET key_id = NULL WHERE key_id = ?1",
        rusqlite::params![key_id],
    )
    .map_err(|e| e.to_string())?;
    let affected = conn
        .execute(
            "DELETE FROM keys WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![key_id, user_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

pub fn decrypt_key(
    db: &Database,
    user_id: &str,
    user_secret: &[u8; 32],
    key_id: &str,
) -> Result<String, String> {
    let conn = db.conn();
    let (encrypted, nonce): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT encrypted, nonce FROM keys WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![key_id, user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "Key not found".to_string())?;

    let nonce: [u8; 12] = nonce.try_into().map_err(|_| "Invalid nonce".to_string())?;

    let dek = crypto::derive_data_key(user_secret, key_id);
    let plaintext = crypto::decrypt(&dek, &nonce, &encrypted).map_err(|e| e.to_string())?;

    let pem = String::from_utf8(plaintext).map_err(|_| "Invalid key data".to_string())?;
    tracing::info!(
        "Key decrypted: id={}, pem_len={}, first_line='{}'",
        key_id,
        pem.len(),
        pem.lines().next().unwrap_or("")
    );
    Ok(pem)
}

fn detect_key_type(pem: &str) -> String {
    if pem.contains("ED25519") {
        "ed25519".to_string()
    } else if pem.contains("EC PRIVATE") {
        "ecdsa".to_string()
    } else {
        "rsa".to_string()
    }
}

fn compute_fingerprint(pem: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(pem.as_bytes());
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hash);
    format!("SHA256:{}", &b64[..27])
}
