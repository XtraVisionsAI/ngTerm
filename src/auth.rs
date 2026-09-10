use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto;
use crate::db::Database;

const TOKEN_EXPIRY_HOURS: i64 = 24;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub exp: usize,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct UserSession {
    #[zeroize(skip)]
    pub user_id: String,
    pub user_secret: [u8; 32],
}

pub struct AuthSessionStore {
    sessions: HashMap<String, UserSession>,
}

impl Default for AuthSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn insert(&mut self, user_id: String, user_secret: [u8; 32]) {
        self.sessions.insert(
            user_id.clone(),
            UserSession {
                user_id,
                user_secret,
            },
        );
    }

    pub fn get_user_secret(&self, user_id: &str) -> Option<[u8; 32]> {
        self.sessions.get(user_id).map(|s| s.user_secret)
    }

    pub fn remove(&mut self, user_id: &str) {
        self.sessions.remove(user_id);
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLoginRequest {
    pub master_key: String,
}

/// Initialize admin on first run: use provided master key or generate one, write hash to .env
pub fn init_admin(data_dir: &str, provided_key: Option<&str>) -> Option<String> {
    let env_path = std::path::Path::new(data_dir).join(".env");

    if env_path.exists() {
        let content = std::fs::read_to_string(&env_path).unwrap_or_default();
        if content.contains("ONEMUX_MASTER_KEY_HASH=") {
            return None;
        }
    }

    let master_key = match provided_key {
        Some(k) => k.to_string(),
        None => crypto::generate_master_key(),
    };
    let hash = crypto::hash_master_key(&master_key);

    let line = format!("ONEMUX_MASTER_KEY_HASH={}\n", hash);
    let content = if env_path.exists() {
        let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
        format!("{}{}", existing, line)
    } else {
        line
    };
    std::fs::write(&env_path, content).expect("Failed to write .env file");

    // Restrict file permissions (unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600));
    }

    Some(master_key)
}

/// Verify admin master key against hash in .env
pub fn verify_admin(data_dir: &str, master_key: &str) -> bool {
    let env_path = std::path::Path::new(data_dir).join(".env");
    let content = std::fs::read_to_string(env_path).unwrap_or_default();

    let stored_hash = content
        .lines()
        .find(|l| l.starts_with("ONEMUX_MASTER_KEY_HASH="))
        .and_then(|l| l.strip_prefix("ONEMUX_MASTER_KEY_HASH="))
        .unwrap_or_default();

    let provided_hash = crypto::hash_master_key(master_key);
    stored_hash == provided_hash
}

/// Read pepper (master_key_hash) from .env file
pub fn read_pepper(data_dir: &str) -> String {
    let env_path = std::path::Path::new(data_dir).join(".env");
    let content = std::fs::read_to_string(env_path).unwrap_or_default();
    content
        .lines()
        .find(|l| l.starts_with("ONEMUX_MASTER_KEY_HASH="))
        .and_then(|l| l.strip_prefix("ONEMUX_MASTER_KEY_HASH="))
        .unwrap_or_default()
        .to_string()
}

/// Read the JWT signing secret from .env, generating and persisting one on
/// first run.
pub fn read_or_generate_jwt_secret(data_dir: &str) -> Vec<u8> {
    let env_path = std::path::Path::new(data_dir).join(".env");
    let prefix = "ONEMUX_JWT_SECRET=";

    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix(prefix) {
                if let Ok(bytes) = hex::decode(val.trim()) {
                    if bytes.len() >= 32 {
                        return bytes;
                    }
                }
            }
        }
    }

    use rand::RngCore;
    let mut secret = vec![0u8; 64];
    rand::thread_rng().fill_bytes(&mut secret);
    let hex_secret = hex::encode(&secret);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&env_path)
        .expect("Failed to open .env for JWT secret");
    use std::io::Write;
    writeln!(file, "{}{}", prefix, hex_secret).expect("Failed to write JWT secret");

    secret
}

/// Create a new user (admin operation)
pub fn create_user(
    db: &Database,
    username: &str,
    default_password: &str,
    pepper: &str,
) -> Result<(String, String), String> {
    let user_id = uuid::Uuid::new_v4().to_string();
    let password_hash = crypto::hash_password(default_password).map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();

    let kdf_salt = crypto::generate_salt();
    let user_secret = crypto::generate_user_secret();
    let wrapping_key = crypto::derive_wrapping_key(default_password, &kdf_salt, pepper);
    let (encrypted_secret, secret_nonce) =
        crypto::wrap_secret(&wrapping_key, &user_secret).map_err(|e| e.to_string())?;

    let conn = db.conn();
    conn.execute(
        "INSERT INTO users (id, username, password, role, kdf_salt, encrypted_secret, secret_nonce, created_at) VALUES (?1, ?2, ?3, 'user', ?4, ?5, ?6, ?7)",
        rusqlite::params![user_id, username, password_hash, kdf_salt.as_slice(), encrypted_secret, secret_nonce.as_slice(), now],
    )
    .map_err(|e| e.to_string())?;

    Ok((user_id, default_password.to_string()))
}

/// User login: verify password, unwrap user_secret
pub fn login(
    db: &Database,
    username: &str,
    password: &str,
    pepper: &str,
    jwt_secret: &[u8],
) -> Result<(String, String, [u8; 32]), String> {
    let conn = db.conn();

    let (user_id, password_hash, kdf_salt, encrypted_secret, secret_nonce): (
        String,
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) = conn
        .query_row(
            "SELECT id, password, kdf_salt, encrypted_secret, secret_nonce FROM users WHERE username = ?1",
            rusqlite::params![username],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(|_| "Invalid username or password".to_string())?;

    if !crypto::verify_password(password, &password_hash) {
        return Err("Invalid username or password".to_string());
    }

    let nonce: [u8; 12] = secret_nonce
        .try_into()
        .map_err(|_| "Invalid nonce".to_string())?;

    let wrapping_key = crypto::derive_wrapping_key(password, &kdf_salt, pepper);
    let user_secret = crypto::unwrap_secret(&wrapping_key, &encrypted_secret, &nonce)
        .map_err(|_| "Failed to decrypt user secret".to_string())?;

    let token = create_token(&user_id, "user", jwt_secret).map_err(|e| e.to_string())?;
    Ok((token, user_id, user_secret))
}

/// Change password: unwrap with old, re-wrap with new
pub fn change_password(
    db: &Database,
    user_id: &str,
    old_password: &str,
    new_password: &str,
    pepper: &str,
) -> Result<(), String> {
    let conn = db.conn();

    let (password_hash, kdf_salt, encrypted_secret, secret_nonce): (
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) = conn
        .query_row(
            "SELECT password, kdf_salt, encrypted_secret, secret_nonce FROM users WHERE id = ?1",
            rusqlite::params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "User not found".to_string())?;

    if !crypto::verify_password(old_password, &password_hash) {
        return Err("Invalid old password".to_string());
    }

    let nonce: [u8; 12] = secret_nonce
        .try_into()
        .map_err(|_| "Invalid nonce".to_string())?;
    let old_wrapping_key = crypto::derive_wrapping_key(old_password, &kdf_salt, pepper);
    let user_secret = crypto::unwrap_secret(&old_wrapping_key, &encrypted_secret, &nonce)
        .map_err(|_| "Failed to decrypt secret".to_string())?;

    // Reuse the connection already held: `db.conn()` is not re-entrant.
    store_credentials(&conn, user_id, new_password, &user_secret, pepper)?;

    Ok(())
}

/// Admin reset password: generates new secret, clears user's SSH keys
pub fn admin_reset_password(
    db: &Database,
    user_id: &str,
    new_password: &str,
    pepper: &str,
) -> Result<(), String> {
    let new_user_secret = crypto::generate_user_secret();
    let conn = db.conn();
    store_credentials(&conn, user_id, new_password, &new_user_secret, pepper)?;

    // Clear all SSH keys for this user (old secret can't decrypt them anymore)
    conn.execute(
        "DELETE FROM keys WHERE user_id = ?1",
        rusqlite::params![user_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn store_credentials(
    conn: &rusqlite::Connection,
    user_id: &str,
    password: &str,
    user_secret: &[u8; 32],
    pepper: &str,
) -> Result<(), String> {
    let password_hash = crypto::hash_password(password).map_err(|e| e.to_string())?;
    let kdf_salt = crypto::generate_salt();
    let wrapping_key = crypto::derive_wrapping_key(password, &kdf_salt, pepper);
    let (encrypted_secret, nonce) =
        crypto::wrap_secret(&wrapping_key, user_secret).map_err(|e| e.to_string())?;

    conn
        .execute(
            "UPDATE users SET password = ?1, kdf_salt = ?2, encrypted_secret = ?3, secret_nonce = ?4 WHERE id = ?5",
            rusqlite::params![password_hash, kdf_salt.as_slice(), encrypted_secret, nonce.as_slice(), user_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Rotate user secret: generate new secret, re-encrypt all keys
#[allow(dead_code)]
pub fn rotate_secret(
    db: &Database,
    user_id: &str,
    old_user_secret: &[u8; 32],
    password: &str,
    pepper: &str,
) -> Result<[u8; 32], String> {
    let new_user_secret = crypto::generate_user_secret();

    // Re-encrypt all SSH keys
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, encrypted, nonce FROM keys WHERE user_id = ?1")
        .map_err(|e| e.to_string())?;

    let key_rows: Vec<(String, Vec<u8>, Vec<u8>)> = stmt
        .query_map(rusqlite::params![user_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    for (key_id, encrypted, nonce_vec) in &key_rows {
        let nonce: [u8; 12] = nonce_vec
            .clone()
            .try_into()
            .map_err(|_| "Invalid nonce".to_string())?;
        let old_dek = crypto::derive_data_key(old_user_secret, key_id);
        let plaintext = crypto::decrypt(&old_dek, &nonce, encrypted).map_err(|e| e.to_string())?;

        let new_dek = crypto::derive_data_key(&new_user_secret, key_id);
        let (new_encrypted, new_nonce) =
            crypto::encrypt(&new_dek, &plaintext).map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE keys SET encrypted = ?1, nonce = ?2 WHERE id = ?3",
            rusqlite::params![new_encrypted, new_nonce.as_slice(), key_id],
        )
        .map_err(|e| e.to_string())?;
    }

    // Re-wrap new secret with password
    let (kdf_salt,): (Vec<u8>,) = conn
        .query_row(
            "SELECT kdf_salt FROM users WHERE id = ?1",
            rusqlite::params![user_id],
            |row| Ok((row.get(0)?,)),
        )
        .map_err(|_| "User not found".to_string())?;

    let wrapping_key = crypto::derive_wrapping_key(password, &kdf_salt, pepper);
    let (new_encrypted_secret, new_nonce) =
        crypto::wrap_secret(&wrapping_key, &new_user_secret).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE users SET encrypted_secret = ?1, secret_nonce = ?2 WHERE id = ?3",
        rusqlite::params![new_encrypted_secret, new_nonce.as_slice(), user_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(new_user_secret)
}

pub fn create_token(
    user_id: &str,
    role: &str,
    secret: &[u8],
) -> Result<String, jsonwebtoken::errors::Error> {
    let expiration = Utc::now()
        .checked_add_signed(chrono::Duration::hours(TOKEN_EXPIRY_HOURS))
        .unwrap()
        .timestamp() as usize;

    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        exp: expiration,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
}

pub fn verify_token(token: &str, secret: &[u8]) -> Option<(String, String)> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret),
        &Validation::default(),
    )
    .ok()?;
    Some((token_data.claims.sub, token_data.claims.role))
}

pub fn list_users(db: &Database) -> Result<Vec<serde_json::Value>, String> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare("SELECT id, username, role, created_at FROM users")
        .map_err(|e| e.to_string())?;

    let users = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "username": row.get::<_, String>(1)?,
                "role": row.get::<_, String>(2)?,
                "createdAt": row.get::<_, String>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(users)
}

pub fn delete_user(db: &Database, user_id: &str) -> Result<bool, String> {
    let conn = db.conn();
    conn.execute(
        "DELETE FROM keys WHERE user_id = ?1",
        rusqlite::params![user_id],
    )
    .map_err(|e| e.to_string())?;
    let affected = conn
        .execute(
            "DELETE FROM users WHERE id = ?1",
            rusqlite::params![user_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}
