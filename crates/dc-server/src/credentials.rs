//! Password and PIN hashing, session tokens, and credential rules.

use anyhow::{Result, anyhow};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Argon2id with m = 19 MiB, t = 2, p = 1. Parameters are stored in each
/// hash string, so they can be raised later without breaking old hashes.
fn hasher() -> Argon2<'static> {
    let params = Params::new(19 * 1024, 2, 1, None).expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_secret(secret: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow!("OS random source failed: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow!("salt encoding: {e}"))?;
    let hash = hasher()
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|e| anyhow!("hashing failed: {e}"))?;
    Ok(hash.to_string())
}

pub fn verify_secret(secret: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|parsed| hasher().verify_password(secret.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

/// 32 bytes from the OS random source, base64url without padding.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random source is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// How a session token is stored: SHA-256, lowercase hex.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Passwords that pass the length rule but are still easy to guess.
const COMMON_PASSWORDS: &[&str] = &[
    "123456789012",
    "1234567890123",
    "password1234",
    "password12345",
    "passwordpassword",
    "qwertyuiop12",
    "qwertyuiop123",
    "qwertyuiopasdf",
    "letmein12345",
    "iloveyou1234",
    "welcome12345",
    "administrator",
    "dentalclarity",
    "dentaloffice1",
    "dentaloffice123",
    "opendental123",
    "frontdesk1234",
    "changeme1234",
    "aaaaaaaaaaaa",
    "111111111111",
];

/// Minimum 12 characters, not a common password, not the practice name.
/// No composition rules (see docs/SECURITY.md).
pub fn check_password(password: &str, practice_name: &str) -> Result<(), &'static str> {
    if password.chars().count() < 12 {
        return Err("Use at least 12 characters.");
    }
    let lower = password.to_lowercase();
    if COMMON_PASSWORDS.contains(&lower.as_str()) {
        return Err("That password is too common. Choose another.");
    }
    let practice: String = practice_name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let squashed: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    if !practice.is_empty() && squashed.contains(&practice) {
        return Err("Don't use the practice name in the password.");
    }
    Ok(())
}

/// 6 to 8 digits, not all the same digit and not a simple run.
pub fn check_pin(pin: &str) -> Result<(), &'static str> {
    if !(6..=8).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err("The PIN must be 6 to 8 digits.");
    }
    let bytes = pin.as_bytes();
    let same = bytes.iter().all(|&b| b == bytes[0]);
    let ascending = bytes.windows(2).all(|w| w[1] == w[0] + 1);
    let descending = bytes.windows(2).all(|w| w[0] == w[1] + 1);
    if same || ascending || descending {
        return Err("That PIN is too easy to guess.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_differ_by_salt() {
        let a = hash_secret("correct horse battery").unwrap();
        let b = hash_secret("correct horse battery").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("$argon2id$"));
        assert!(verify_secret("correct horse battery", &a));
        assert!(!verify_secret("wrong horse battery", &a));
        assert!(!verify_secret("anything", "not a hash"));
    }

    #[test]
    fn tokens_are_unique_and_url_safe() {
        let t = new_token();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(t, new_token());
        assert_eq!(token_hash(&t).len(), 64);
    }

    #[test]
    fn password_rules() {
        assert!(check_password("short", "X").is_err());
        assert!(check_password("Password1234", "X").is_err());
        assert!(check_password("maplegrove-2026!", "Maple Grove").is_err());
        assert!(check_password("tooth fairy on duty", "Maple Grove").is_ok());
    }

    #[test]
    fn pin_rules() {
        assert!(check_pin("12345").is_err());
        assert!(check_pin("123456").is_err());
        assert!(check_pin("654321").is_err());
        assert!(check_pin("000000").is_err());
        assert!(check_pin("12a456").is_err());
        assert!(check_pin("480913").is_ok());
    }
}
