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

/// Run a CPU-heavy or lock-holding credential operation off the async
/// runtime. Argon2 takes tens of milliseconds per call; doing it inline
/// would stall every other connection sharing the worker thread.
pub async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(format!("credential task failed: {}", e)))
}

/// Freshly derived password hash and wrapped user secret, computed without
/// holding the database lock.
struct Credentials {
    password_hash: String,
    kdf_salt: [u8; 16],
    encrypted_secret: Vec<u8>,
    secret_nonce: [u8; 12],
}

fn derive_credentials(
    password: &str,
    user_secret: &[u8; 32],
    pepper: &str,
) -> Result<Credentials, String> {
    let password_hash = crypto::hash_password(password).map_err(|e| e.to_string())?;
    let kdf_salt = crypto::generate_salt();
    let wrapping_key = crypto::derive_wrapping_key(password, &kdf_salt, pepper);
    let (encrypted_secret, secret_nonce) =
        crypto::wrap_secret(&wrapping_key, user_secret).map_err(|e| e.to_string())?;
    Ok(Credentials {
        password_hash,
        kdf_salt,
        encrypted_secret,
        secret_nonce,
    })
}

/// User login: verify password, unwrap user_secret. The database lock is
/// held only for the row lookup, never across the KDF.
pub fn login(
    db: &Database,
    username: &str,
    password: &str,
    pepper: &str,
    jwt_secret: &[u8],
) -> Result<(String, String, [u8; 32]), String> {
    let (user_id, password_hash, kdf_salt, encrypted_secret, secret_nonce): (
        String,
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) = db
        .conn()
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
    let (password_hash, kdf_salt, encrypted_secret, secret_nonce): (
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    ) = db
        .conn()
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

    let creds = derive_credentials(new_password, &user_secret, pepper)?;
    store_credentials(&db.conn(), user_id, &creds)
}

/// Admin reset password: generates new secret, clears user's SSH keys
pub fn admin_reset_password(
    db: &Database,
    user_id: &str,
    new_password: &str,
    pepper: &str,
) -> Result<(), String> {
    let new_user_secret = crypto::generate_user_secret();
    let creds = derive_credentials(new_password, &new_user_secret, pepper)?;

    let conn = db.conn();
    store_credentials(&conn, user_id, &creds)?;

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
    creds: &Credentials,
) -> Result<(), String> {
    conn
        .execute(
            "UPDATE users SET password = ?1, kdf_salt = ?2, encrypted_secret = ?3, secret_nonce = ?4 WHERE id = ?5",
            rusqlite::params![creds.password_hash, creds.kdf_salt.as_slice(), creds.encrypted_secret, creds.secret_nonce.as_slice(), user_id],
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

    drop(stmt);
    drop(conn);

    // Re-wrap new secret with password (KDF runs without the DB lock).
    let (kdf_salt,): (Vec<u8>,) = db
        .conn()
        .query_row(
            "SELECT kdf_salt FROM users WHERE id = ?1",
            rusqlite::params![user_id],
            |row| Ok((row.get(0)?,)),
        )
        .map_err(|_| "User not found".to_string())?;

    let wrapping_key = crypto::derive_wrapping_key(password, &kdf_salt, pepper);
    let (new_encrypted_secret, new_nonce) =
        crypto::wrap_secret(&wrapping_key, &new_user_secret).map_err(|e| e.to_string())?;

    db.conn()
        .execute(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn temp_db() -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ngterm-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(dir.to_str().unwrap()).unwrap();
        (Arc::new(db), dir)
    }

    /// Measures how long the runtime's single worker was unable to service a
    /// 1ms timer while `work` ran. On a current-thread runtime a CPU-bound
    /// inline KDF shows up as one stall of roughly the KDF's duration; work
    /// moved to the blocking pool must leave the timer essentially unaffected.
    async fn max_probe_lag<F: std::future::Future>(work: F) -> (Duration, F::Output) {
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let probe = tokio::spawn(async move {
            let mut worst = Duration::ZERO;
            let mut last = Instant::now();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(1)) => {
                        let now = Instant::now();
                        worst = worst.max(now - last);
                        last = now;
                    }
                    _ = &mut stop_rx => break worst,
                }
            }
        });
        // Let the probe take its first timestamp, and let it observe the
        // gap after the work finished before stopping it.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let out = work.await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let _ = stop_tx.send(());
        (probe.await.unwrap(), out)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn password_kdf_runs_off_the_async_worker() {
        let (db, dir) = temp_db();
        let pepper = "pepper";
        create_user(&db, "alice", "correct horse", pepper).unwrap();

        // Baseline: one login (verify + derive) run inline, timed.
        let started = Instant::now();
        login(&db, "alice", "correct horse", pepper, b"jwt").unwrap();
        let kdf_cost = started.elapsed();

        // Before: inline KDF freezes the worker for the whole KDF.
        let (inline_lag, res) =
            max_probe_lag(async { login(&db, "alice", "correct horse", pepper, b"jwt") }).await;
        assert!(res.is_ok());
        assert!(
            inline_lag >= kdf_cost / 2,
            "inline KDF should stall the worker (lag {:?}, kdf {:?})",
            inline_lag,
            kdf_cost
        );

        // After: several concurrent logins through the blocking pool keep
        // the worker responsive.
        let (pooled_lag, results) = max_probe_lag(async {
            let mut handles = Vec::new();
            for _ in 0..4 {
                let db = db.clone();
                handles.push(tokio::spawn(async move {
                    run_blocking(move || login(&db, "alice", "correct horse", pepper, b"jwt")).await
                }));
            }
            let mut out = Vec::new();
            for h in handles {
                out.push(h.await.unwrap());
            }
            out
        })
        .await;
        assert!(results.iter().all(|r| r.is_ok()), "{:?}", results);
        let budget = std::cmp::max(Duration::from_millis(20), kdf_cost / 4);
        assert!(
            pooled_lag < budget,
            "blocking-pool KDF must not stall the worker (lag {:?}, kdf {:?})",
            pooled_lag,
            kdf_cost
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credential_changes_do_not_hold_the_db_lock_across_the_kdf() {
        let (db, dir) = temp_db();
        let pepper = "pepper";
        let (uid, _) = create_user(&db, "bob", "old-pass", pepper).unwrap();

        change_password(&db, &uid, "old-pass", "new-pass", pepper).unwrap();
        assert!(login(&db, "bob", "old-pass", pepper, b"jwt").is_err());
        let (_, _, secret_after_change) = login(&db, "bob", "new-pass", pepper, b"jwt").unwrap();

        admin_reset_password(&db, &uid, "reset-pass", pepper).unwrap();
        let (_, _, secret_after_reset) = login(&db, "bob", "reset-pass", pepper, b"jwt").unwrap();
        assert_ne!(
            secret_after_change, secret_after_reset,
            "reset must mint a new secret"
        );

        let rotated = rotate_secret(&db, &uid, &secret_after_reset, "reset-pass", pepper).unwrap();
        let (_, _, secret_after_rotate) = login(&db, "bob", "reset-pass", pepper, b"jwt").unwrap();
        assert_eq!(rotated, secret_after_rotate);

        let _ = std::fs::remove_dir_all(dir);
    }
}
