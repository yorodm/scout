//! Credential and token primitives.
//!
//! Passwords use Argon2id; opaque tokens are generated from the OS and stored
//! only as SHA-256 hashes (Additional Constraint: hashed credentials). These
//! primitives are consumed by the persistent store and session API as they are
//! wired in; until then the module is compiled and unit-tested on its own.

#![allow(dead_code)]

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Hash a password with Argon2id. The returned string is the PHC encoding, so
/// the salt and parameters travel with the hash.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| format!("could not hash password: {e}"))
}

/// Constant-time verification performed by Argon2 itself.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A 256-bit opaque token, URL-safe and unpadded so it can be used as a query
/// value or header value. Shown once; only its hash is stored.
pub fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 hex digest used for sessions, invitations, ingest keys, and recovery
/// codes.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// First `len` characters of a token, for identifying a key in the UI without
/// exposing the secret.
pub fn token_prefix(token: &str, len: usize) -> String {
    token.chars().take(len).collect()
}

/// A single-use recovery code in `XXXXX-XXXXX` form. Only the hash is stored.
pub fn new_recovery_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let bytes: [u8; 10] = rand::random();
    let code: String = bytes
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &code[..5], &code[5..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_round_trip_and_rejection() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong password", &hash));
        assert!(!verify_password("anything", "not a phc hash"));
    }

    #[test]
    fn tokens_are_opaque_and_hashed() {
        let first = new_token();
        let second = new_token();
        assert_ne!(first, second);
        assert!(first.len() >= 40);
        assert_eq!(hash_token(&first), hash_token(&first));
        assert_ne!(hash_token(&first), hash_token(&second));
        assert_eq!(hash_token("abc").len(), 64);
    }

    #[test]
    fn recovery_codes_have_a_readable_shape() {
        let code = new_recovery_code();
        assert_eq!(code.len(), 11);
        assert_eq!(code.as_bytes()[5], b'-');
        assert_ne!(code, new_recovery_code());
    }
}
