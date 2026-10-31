//! Credential and token primitives.
//!
//! Passwords use Argon2id; opaque tokens are generated from the OS and stored
//! only as SHA-256 hashes (Additional Constraint: hashed credentials). This
//! module also owns the idempotent deployment-operator bootstrap.

use crate::config::Config;
use crate::models::{RecoveryCode, User};
use crate::store::Store;
use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

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

/// Outcome of the startup bootstrap.
#[derive(Debug, PartialEq, Eq)]
pub enum BootstrapOutcome {
    /// A new operator was created from the environment.
    Created(String),
    /// An operator already existed; its password was left untouched.
    Existing,
}

/// Create the deployment operator when none exists.
///
/// FR-030: values are read from the environment only on first start, an existing
/// operator's password is never reset, and startup fails with an actionable
/// message when no operator exists and either value is missing or blank.
pub fn bootstrap_operator(store: &mut Store, config: &Config) -> Result<BootstrapOutcome, String> {
    if let Some(existing) = store.users.values().find(|user| user.is_operator) {
        if config.operator_email.is_some() || config.operator_password.is_some() {
            tracing::warn!(
                operator = %existing.email,
                "deployment operator already exists; bootstrap values are ignored and the password is unchanged"
            );
        }
        return Ok(BootstrapOutcome::Existing);
    }

    let email = config
        .operator_email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    // Passwords are compared verbatim, so only reject an absent or blank value.
    let password = config
        .operator_password
        .as_deref()
        .filter(|value| !value.trim().is_empty());

    let (Some(email), Some(password)) = (email, password) else {
        return Err(
            "no deployment operator exists: set SCOUT_OPERATOR_EMAIL and \
SCOUT_OPERATOR_PASSWORD (neither may be blank) and restart"
                .into(),
        );
    };

    let email = email.to_ascii_lowercase();
    let password_hash = hash_password(password)
        .map_err(|e| format!("could not hash the operator password: {e}"))?;
    let user = User {
        id: Uuid::new_v4(),
        email: email.clone(),
        name: "Deployment operator".into(),
        password_hash: Some(password_hash),
        is_operator: true,
        disabled_at: None,
    };
    store.users.insert(user.id, user.clone());
    store.users_by_email.insert(email.clone(), user.id);
    crate::audit::record(
        store,
        None,
        None,
        "system",
        "operator.bootstrapped",
        Some(email.clone()),
    );
    Ok(BootstrapOutcome::Created(email))
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

/// Create a local account with a hashed password. Callers must have already
/// authorised the creation (invitation acceptance or initial-admin setup).
pub fn create_local_account(
    store: &mut Store,
    email: &str,
    name: &str,
    password: &str,
) -> Result<User, String> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err("a valid email address is required".into());
    }
    if store.user_by_email(&email).is_some() {
        return Err("an account with that email already exists".into());
    }
    if password.is_empty() {
        return Err("a password is required for a new account".into());
    }
    let password_hash = hash_password(password)?;
    let user = User {
        id: Uuid::new_v4(),
        email: email.clone(),
        name: name.trim().to_string(),
        password_hash: Some(password_hash),
        is_operator: false,
        disabled_at: None,
    };
    store.users.insert(user.id, user.clone());
    store.users_by_email.insert(email, user.id);
    Ok(user)
}

/// Issue a fresh set of recovery codes, revoking every unused code first. The
/// plaintext codes are returned once; only hashes are stored.
pub fn issue_recovery_codes(store: &mut Store, user_id: Uuid, count: usize) -> Vec<String> {
    let now = Utc::now();
    for code in &mut store.recovery_codes {
        if code.user_id == user_id && code.used_at.is_none() && code.revoked_at.is_none() {
            code.revoked_at = Some(now);
        }
    }
    let mut plain = Vec::with_capacity(count);
    for _ in 0..count {
        let code = new_recovery_code();
        store.recovery_codes.push(RecoveryCode {
            id: Uuid::new_v4(),
            user_id,
            code_hash: hash_token(&code),
            created_at: now,
            used_at: None,
            revoked_at: None,
        });
        plain.push(code);
    }
    plain
}

/// Consume a recovery code and set a new password. Any failure is reported as the
/// same error so codes and accounts cannot be probed.
pub fn redeem_recovery_code(
    store: &mut Store,
    email: &str,
    code: &str,
    new_password: &str,
) -> Result<Uuid, String> {
    let user_id = store
        .user_by_email(email)
        .map(|user| user.id)
        .ok_or_else(|| "invalid email or recovery code".to_string())?;
    if new_password.is_empty() {
        return Err("a new password is required".into());
    }
    let code_hash = hash_token(code.trim());
    let now = Utc::now();
    let entry = store.recovery_codes.iter_mut().find(|entry| {
        entry.user_id == user_id
            && entry.code_hash == code_hash
            && entry.used_at.is_none()
            && entry.revoked_at.is_none()
    });
    let Some(entry) = entry else {
        return Err("invalid email or recovery code".into());
    };
    let password_hash = hash_password(new_password)?;
    entry.used_at = Some(now);
    if let Some(user) = store.users.get_mut(&user_id) {
        user.password_hash = Some(password_hash);
    }
    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(email: Option<&str>, password: Option<&str>) -> Config {
        Config {
            port: 8080,
            self_url: "http://127.0.0.1:1".into(),
            ingest_max_in_flight: 256,
            retention_days: 30,
            confidence_threshold: 0.5,
            session_ttl_days: 7,
            operator_email: email.map(str::to_string),
            operator_password: password.map(str::to_string),
            database_url: None,
            s3_endpoint: None,
            s3_bucket: None,
            demo_mode: false,
        }
    }

    fn operator_hash(store: &Store) -> String {
        store
            .user_by_email("ops@example.com")
            .expect("operator account")
            .password_hash
            .clone()
            .expect("operator password hash")
    }

    #[test]
    fn operator_bootstrap_is_idempotent_and_never_resets_the_password() {
        let (mut store, _raw) = crate::store::seed("http://127.0.0.1:1");

        let outcome = bootstrap_operator(
            &mut store,
            &config(Some(" Ops@Example.com "), Some("first-password")),
        )
        .expect("bootstrap the operator");
        assert_eq!(outcome, BootstrapOutcome::Created("ops@example.com".into()));
        assert_eq!(
            store.users.values().filter(|user| user.is_operator).count(),
            1
        );
        let hash = operator_hash(&store);
        assert!(verify_password("first-password", &hash));
        assert!(store
            .audit
            .iter()
            .any(|entry| entry.action == "operator.bootstrapped" && entry.org_id.is_none()));

        // A later start with different values must not reset the password.
        let outcome = bootstrap_operator(
            &mut store,
            &config(Some("ops@example.com"), Some("second-password")),
        )
        .expect("bootstrap again");
        assert_eq!(outcome, BootstrapOutcome::Existing);
        let hash = operator_hash(&store);
        assert!(verify_password("first-password", &hash));
        assert!(!verify_password("second-password", &hash));
        assert_eq!(
            store.users.values().filter(|user| user.is_operator).count(),
            1
        );
    }

    #[test]
    fn missing_or_blank_bootstrap_values_fail_without_creating_an_operator() {
        let (mut store, _raw) = crate::store::seed("http://127.0.0.1:1");

        for (email, password) in [
            (None, None),
            (Some("  "), Some("secret")),
            (Some("ops@example.com"), Some("   ")),
        ] {
            let error = bootstrap_operator(&mut store, &config(email, password))
                .expect_err("missing bootstrap values must fail startup");
            assert!(error.contains("SCOUT_OPERATOR_EMAIL"), "error: {error}");
            assert!(error.contains("SCOUT_OPERATOR_PASSWORD"), "error: {error}");
        }
        assert!(store.users.values().all(|user| !user.is_operator));
        assert!(store.audit.is_empty());
    }

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
