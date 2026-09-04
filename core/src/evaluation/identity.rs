//! Stable identities and digest helpers used by the evaluation substrate.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const EXECUTION_TARGET_SCHEMA_V1: &str = "a3s.code.execution-target.v1";
pub const EXECUTION_FRAME_SCHEMA_V1: &str = "a3s.code.execution-frame.v1";
pub const EVALUATION_MAX_ID_BYTES: usize = 256;
const MAX_BRANCH_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityError {
    #[error("unsupported identity schema")]
    UnsupportedSchema,
    #[error("identity field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("identity digest is invalid")]
    InvalidDigest,
    #[error("identity serialization failed: {0}")]
    Serialization(String),
}

/// Session/run identity used by all evaluation records.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTargetV1 {
    pub schema: String,
    pub session_id: String,
    pub run_id: String,
}

impl ExecutionTargetV1 {
    pub const fn schema() -> &'static str {
        EXECUTION_TARGET_SCHEMA_V1
    }

    pub fn new(session_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self {
            schema: EXECUTION_TARGET_SCHEMA_V1.to_string(),
            session_id: session_id.into(),
            run_id: run_id.into(),
        }
    }

    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.schema != EXECUTION_TARGET_SCHEMA_V1 {
            return Err(IdentityError::UnsupportedSchema);
        }
        validate_id("session_id", &self.session_id)?;
        validate_id("run_id", &self.run_id)
    }

    pub fn digest(&self) -> Result<String, IdentityError> {
        self.validate()?;
        digest_json("a3s.code.execution-target.identity.v1", self)
            .map_err(|error| IdentityError::Serialization(error.to_string()))
    }
}

/// Runtime frame that records parentage without taking ownership of Cloud
/// checkpoint/fork lineage.  A child evaluation may point at its parent run;
/// hosts remain responsible for business lineage and authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFrameV1 {
    pub schema: String,
    pub target: ExecutionTargetV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ExecutionTargetV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub generation: u64,
}

impl ExecutionFrameV1 {
    pub fn root(target: ExecutionTargetV1) -> Self {
        Self {
            schema: EXECUTION_FRAME_SCHEMA_V1.to_string(),
            target,
            parent: None,
            branch: None,
            generation: 0,
        }
    }

    pub fn child(target: ExecutionTargetV1, parent: ExecutionTargetV1) -> Self {
        Self {
            schema: EXECUTION_FRAME_SCHEMA_V1.to_string(),
            target,
            parent: Some(parent),
            branch: None,
            generation: 0,
        }
    }

    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.schema != EXECUTION_FRAME_SCHEMA_V1 {
            return Err(IdentityError::UnsupportedSchema);
        }
        self.target.validate()?;
        if let Some(parent) = &self.parent {
            parent.validate()?;
            if parent == &self.target {
                return Err(IdentityError::InvalidField("parent"));
            }
        }
        if let Some(branch) = &self.branch {
            if branch.is_empty()
                || branch.len() > MAX_BRANCH_BYTES
                || branch.contains('\0')
                || branch.lines().count() != 1
            {
                return Err(IdentityError::InvalidField("branch"));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, IdentityError> {
        self.validate()?;
        digest_json("a3s.code.execution-frame.identity.v1", self)
            .map_err(|error| IdentityError::Serialization(error.to_string()))
    }
}

/// Cursor for a run-local append-only event/fact stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventCursorV1 {
    pub sequence: u64,
}

impl EventCursorV1 {
    pub const fn new(sequence: u64) -> Self {
        Self { sequence }
    }

    pub fn next(self) -> Option<Self> {
        self.sequence.checked_add(1).map(Self::new)
    }
}

pub fn digest_bytes(domain: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    let digest = hasher.finalize();
    format!("sha256:{digest:x}")
}

pub fn digest_json<T: Serialize>(domain: &str, value: &T) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(value)?;
    Ok(digest_bytes(domain, &bytes))
}

pub fn validate_digest(value: &str) -> Result<(), IdentityError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(IdentityError::InvalidDigest);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(IdentityError::InvalidDigest);
    }
    Ok(())
}

fn validate_id(field: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty()
        || value.len() > EVALUATION_MAX_ID_BYTES
        || value.contains('\0')
        || value.lines().count() != 1
    {
        return Err(IdentityError::InvalidField(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_digest_is_domain_separated_and_canonical() {
        let target = ExecutionTargetV1::new("session-1", "run-1");
        let first = target.digest().unwrap();
        let second = digest_json("a3s.code.execution-target.identity.v1", &target).unwrap();
        assert_eq!(first, second);
        assert_ne!(
            digest_bytes("domain-a", b"same"),
            digest_bytes("domain-b", b"same")
        );
        assert!(validate_digest(&first).is_ok());
        assert!(validate_digest(&first.to_ascii_uppercase()).is_err());
    }

    #[test]
    fn frame_rejects_self_parent_and_cursor_overflow() {
        let target = ExecutionTargetV1::new("session-1", "run-1");
        let frame = ExecutionFrameV1 {
            schema: EXECUTION_FRAME_SCHEMA_V1.to_string(),
            target: target.clone(),
            parent: Some(target),
            branch: None,
            generation: 0,
        };
        assert!(matches!(
            frame.validate(),
            Err(IdentityError::InvalidField("parent"))
        ));
        assert_eq!(EventCursorV1::new(u64::MAX).next(), None);
    }
}
