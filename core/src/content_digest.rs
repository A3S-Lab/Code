//! Shared domain-separated SHA-256 digests.
//!
//! These helpers are used by the evaluation substrate and by non-evaluation
//! harness paths (store WAL, orchestration checkpoints, Flow decisions).
//! Keeping them outside `evaluation` lets Advanced evaluation stay optional
//! without forcing digest callers into that module graph.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Domain-separated SHA-256 digest encoded as `sha256:<hex>`.
pub fn digest_bytes(domain: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    let digest = hasher.finalize();
    format!("sha256:{digest:x}")
}

/// JSON-serialize `value` then digest the bytes under `domain`.
pub fn digest_json<T: Serialize>(domain: &str, value: &T) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(value)?;
    Ok(digest_bytes(domain, &bytes))
}

/// Reject digests that are not lowercase `sha256:` + 64 hex characters.
pub fn validate_digest(value: &str) -> Result<(), InvalidDigest> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(InvalidDigest);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(InvalidDigest);
    }
    Ok(())
}

/// Marker error for an ill-formed `sha256:<hex>` digest string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDigest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_changes_digest() {
        let left = digest_bytes("a.v1", b"payload");
        let right = digest_bytes("b.v1", b"payload");
        assert_ne!(left, right);
        assert!(left.starts_with("sha256:"));
        assert_eq!(left.len(), "sha256:".len() + 64);
    }

    #[test]
    fn validate_digest_accepts_canonical_form() {
        let digest = digest_bytes("a.v1", b"payload");
        assert!(validate_digest(&digest).is_ok());
        assert!(validate_digest(&digest.to_ascii_uppercase()).is_err());
        assert!(validate_digest("not-a-digest").is_err());
    }

    #[test]
    fn digest_json_round_trips_structured_values() {
        #[derive(Serialize)]
        struct Payload<'a> {
            name: &'a str,
            count: u32,
        }
        let digest = digest_json(
            "test.domain",
            &Payload {
                name: "alpha",
                count: 3,
            },
        )
        .unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert_eq!(
            digest,
            digest_json(
                "test.domain",
                &Payload {
                    name: "alpha",
                    count: 3
                }
            )
            .unwrap()
        );
        assert_ne!(
            digest,
            digest_json(
                "other.domain",
                &Payload {
                    name: "alpha",
                    count: 3
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn validate_digest_rejects_malformed_hex() {
        let digest = digest_bytes("a.v1", b"payload");
        let short = format!("sha256:{}", "a".repeat(63));
        let long = format!("sha256:{}", "a".repeat(65));
        let non_hex = format!("sha256:{}", "g".repeat(64));
        assert!(validate_digest(&digest).is_ok());
        assert!(validate_digest(&short).is_err());
        assert!(validate_digest(&long).is_err());
        assert!(validate_digest(&non_hex).is_err());
        assert!(validate_digest("sha256:").is_err());
    }
}
