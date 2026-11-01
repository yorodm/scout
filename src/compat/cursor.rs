//! Opaque, query-bound pagination cursors.
//!
//! A cursor is base64url(JSON) with the format version, a fingerprint of the
//! query that produced it, the sort mode, the last sort key, and the last issue
//! id (the deterministic tiebreak). Cursors carry no authority: they only
//! position the next page.

use crate::compat::error::CompatError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub v: u8,
    /// Fingerprint of the query tuple the cursor belongs to.
    pub q: String,
    /// Sort mode, so a cursor cannot be replayed against a different ordering.
    pub s: String,
    /// Last sort key on this page (decimal milliseconds or event count).
    pub k: String,
    /// Last issue id on this page; empty for value-based cursors.
    pub i: String,
}

/// Stable 16-hex fingerprint of the normalised query tuple.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn encode(cursor: &Cursor) -> String {
    let bytes = serde_json::to_vec(cursor).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decode(raw: &str) -> Result<Cursor, CompatError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| CompatError::BadRequest("invalid cursor".into()))?;
    let cursor: Cursor = serde_json::from_slice(&bytes)
        .map_err(|_| CompatError::BadRequest("invalid cursor".into()))?;
    if cursor.v != 1 {
        return Err(CompatError::BadRequest("invalid cursor".into()));
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Cursor {
        Cursor {
            v: 1,
            q: fingerprint(&["is:resolved", "date", "", "", "org", "project"]),
            s: "date".into(),
            k: "1761984000000".into(),
            i: "3f2b1c9e-8a5d-4a0e-9f1c-2b6d7e8f9a01".into(),
        }
    }

    #[test]
    fn round_trips() {
        let decoded = decode(&encode(&sample())).expect("decode");
        assert_eq!(decoded, sample());
    }

    #[test]
    fn rejects_garbage_and_versions() {
        assert!(decode("not base64!").is_err());
        let mut cursor = sample();
        cursor.v = 2;
        assert!(decode(&encode(&cursor)).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_discriminating() {
        assert_eq!(
            fingerprint(&["a", "b"]),
            fingerprint(&["a", "b"]),
            "same input must fingerprint identically"
        );
        assert_ne!(fingerprint(&["a", "b"]), fingerprint(&["a", "c"]));
        assert_eq!(fingerprint(&["a"]).len(), 16);
    }
}
