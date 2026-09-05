//! Stable identities for replayable execution boundaries.
//!
//! The identity is deliberately content-addressed and domain-separated. It
//! is suitable for deduplication and fencing, but it does not itself claim a
//! lease or persist an outcome; those responsibilities remain with the
//! caller's ledger.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

pub const EXECUTION_IDENTITY_SCHEMA_V1: &str = "a3s.code.execution-identity.v1";
pub const MODEL_CALL_IDENTITY_DOMAIN_V1: &str = "a3s.code.model-call.identity.v1";
pub const TOOL_INVOCATION_IDENTITY_DOMAIN_V1: &str = "a3s.code.tool-invocation.identity.v1";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutionIdentityError {
    #[error("execution identity domain is empty or invalid")]
    InvalidDomain,
    #[error("execution identity serialization failed: {0}")]
    Serialization(String),
    #[error("execution identity digest is invalid")]
    InvalidDigest,
    #[error("execution identity digest does not match the value")]
    DigestMismatch,
}

/// A portable, domain-separated content identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentityV1 {
    pub schema: String,
    pub domain: String,
    pub digest: String,
}

impl ExecutionIdentityV1 {
    pub fn derive<T: Serialize>(
        domain: impl Into<String>,
        value: &T,
    ) -> Result<Self, ExecutionIdentityError> {
        let domain = domain.into();
        validate_domain(&domain)?;
        let canonical = serde_json::to_value(value)
            .map(canonicalize)
            .map_err(|error| ExecutionIdentityError::Serialization(error.to_string()))?;
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|error| ExecutionIdentityError::Serialization(error.to_string()))?;
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        let digest = format!("sha256:{:x}", hasher.finalize());
        Ok(Self {
            schema: EXECUTION_IDENTITY_SCHEMA_V1.to_string(),
            domain,
            digest,
        })
    }

    pub fn validate(&self) -> Result<(), ExecutionIdentityError> {
        if self.schema != EXECUTION_IDENTITY_SCHEMA_V1 {
            return Err(ExecutionIdentityError::InvalidDomain);
        }
        validate_domain(&self.domain)?;
        let Some(hex) = self.digest.strip_prefix("sha256:") else {
            return Err(ExecutionIdentityError::InvalidDigest);
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ExecutionIdentityError::InvalidDigest);
        }
        Ok(())
    }

    pub fn key(&self) -> &str {
        &self.digest
    }

    pub fn validate_for<T: Serialize>(&self, value: &T) -> Result<(), ExecutionIdentityError> {
        self.validate()?;
        let expected = Self::derive(&self.domain, value)?;
        if expected.digest != self.digest {
            return Err(ExecutionIdentityError::DigestMismatch);
        }
        Ok(())
    }
}

fn canonicalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize).collect())
        }
        serde_json::Value::Object(values) => {
            let values = values
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>();
            serde_json::Value::Object(values.into_iter().collect())
        }
        value => value,
    }
}

fn validate_domain(domain: &str) -> Result<(), ExecutionIdentityError> {
    if domain.is_empty()
        || domain.len() > 128
        || domain.contains('\0')
        || domain.lines().count() != 1
        || !domain.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
    {
        return Err(ExecutionIdentityError::InvalidDomain);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_domain_separated_and_replay_stable() {
        let value = serde_json::json!({"name": "read", "args": {"path": "src/lib.rs"}});
        let first =
            ExecutionIdentityV1::derive(TOOL_INVOCATION_IDENTITY_DOMAIN_V1, &value).unwrap();
        let second =
            ExecutionIdentityV1::derive(TOOL_INVOCATION_IDENTITY_DOMAIN_V1, &value).unwrap();
        let other = ExecutionIdentityV1::derive(MODEL_CALL_IDENTITY_DOMAIN_V1, &value).unwrap();

        assert_eq!(first, second);
        assert_ne!(first.digest, other.digest);
        first.validate().unwrap();
        first.validate_for(&value).unwrap();

        let left = serde_json::json!({"a": 1, "b": 2});
        let right = serde_json::json!({"b": 2, "a": 1});
        assert_eq!(
            ExecutionIdentityV1::derive("a3s.test", &left).unwrap(),
            ExecutionIdentityV1::derive("a3s.test", &right).unwrap()
        );
    }

    #[test]
    fn identity_rejects_malformed_domains_and_digests() {
        assert!(matches!(
            ExecutionIdentityV1::derive("Bad Domain", &"value"),
            Err(ExecutionIdentityError::InvalidDomain)
        ));
        let mut identity = ExecutionIdentityV1::derive("a3s.test", &"value").unwrap();
        identity.digest = "sha256:ABC".to_string();
        assert!(matches!(
            identity.validate(),
            Err(ExecutionIdentityError::InvalidDigest)
        ));
    }

    #[test]
    fn identity_validation_detects_content_tampering() {
        let value = serde_json::json!({"a": 1, "b": {"c": true}});
        let identity = ExecutionIdentityV1::derive("a3s.test", &value).unwrap();
        let changed = serde_json::json!({"a": 2, "b": {"c": true}});
        assert!(matches!(
            identity.validate_for(&changed),
            Err(ExecutionIdentityError::DigestMismatch)
        ));
    }
}
