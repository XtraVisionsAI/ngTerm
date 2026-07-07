use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{self, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

const KDF_INFO: &[u8] = b"onemux-key-enc";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("argon2 error: {0}")]
    Argon2(String),
}

pub fn generate_random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

pub fn generate_nonce() -> [u8; 12] {
    generate_random_bytes::<12>()
}

pub fn generate_salt() -> [u8; 16] {
    generate_random_bytes::<16>()
}

pub fn generate_user_secret() -> [u8; 32] {
    generate_random_bytes::<32>()
}

/// Derive a wrapping key from password + pepper + salt (for encrypting user_secret)
/// Pepper (master_key_hash) ensures DB alone is insufficient to brute-force passwords.
pub fn derive_wrapping_key(password: &str, salt: &[u8], pepper: &str) -> [u8; 32] {
    let input = format!("{}{}", password, pepper);
    let mut key = [0u8; 32];
    let argon2 = Argon2::default();
    argon2
        .hash_password_into(input.as_bytes(), salt, &mut key)
        .expect("Argon2 key derivation failed");
    key
}

/// Derive a data encryption key from user_secret + key_id (for encrypting SSH keys)
pub fn derive_data_key(user_secret: &[u8; 32], key_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(key_id.as_bytes()), user_secret);
    let mut dek = [0u8; 32];
    hk.expand(KDF_INFO, &mut dek).expect("HKDF expand failed");
    dek
}

/// AES-256-GCM encrypt
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    let nonce_bytes = generate_nonce();
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::EncryptionFailed)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::EncryptionFailed)?;
    Ok((ciphertext, nonce_bytes))
}

/// AES-256-GCM decrypt
pub fn decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::DecryptionFailed)?;
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::DecryptionFailed)
}

/// Encrypt user_secret with wrapping_key
pub fn wrap_secret(
    wrapping_key: &[u8; 32],
    user_secret: &[u8; 32],
) -> Result<(Vec<u8>, [u8; 12]), CryptoError> {
    encrypt(wrapping_key, user_secret)
}

/// Decrypt user_secret with wrapping_key
pub fn unwrap_secret(
    wrapping_key: &[u8; 32],
    encrypted_secret: &[u8],
    nonce: &[u8; 12],
) -> Result<[u8; 32], CryptoError> {
    let plaintext = decrypt(wrapping_key, nonce, encrypted_secret)?;
    plaintext
        .try_into()
        .map_err(|_| CryptoError::DecryptionFailed)
}

/// Hash password for storage (login verification)
pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    let salt = argon2::password_hash::SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| CryptoError::Argon2(e.to_string()))
}

/// Verify password against stored hash
pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

/// Generate a random master key string (base64 encoded)
pub fn generate_master_key() -> String {
    let bytes: [u8; 32] = generate_random_bytes();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// Hash master key for storage comparison (SHA-256)
pub fn hash_master_key(master_key: &str) -> String {
    use sha2::Digest;
    let hash = Sha256::digest(master_key.as_bytes());
    hex::encode(hash)
}
