//! Host-selected work guidance. It never grants permissions or completion evidence.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use thiserror::Error;

pub(crate) const WORK_POLICY_SCHEMA: &str = "a3s.code.work-policy.v1";
const MAX_ID_BYTES: usize = 128;
const MAX_INSTRUCTION_BYTES: usize = 4096;

/// A validated policy value; admission and runtime inheritance belong to the session.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct WorkPolicyV1 {
    id: String,
    revision: u32,
    instructions: String,
}

impl WorkPolicyV1 {
    pub fn new(
        id: impl Into<String>,
        revision: u32,
        instructions: impl Into<String>,
    ) -> Result<Self, WorkPolicyError> {
        let id = id.into();
        let instructions = instructions.into();
        if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(char::is_control) {
            return Err(WorkPolicyError::Invalid {
                field: "id",
                reason: "must contain 1–128 UTF-8 bytes and no control characters",
            });
        }
        if revision == 0 {
            return Err(WorkPolicyError::Invalid {
                field: "revision",
                reason: "must be greater than zero",
            });
        }
        if instructions.trim().is_empty()
            || instructions.len() > MAX_INSTRUCTION_BYTES
            || instructions.contains('\0')
        {
            return Err(WorkPolicyError::Invalid {
                field: "instructions",
                reason: "must be nonblank, at most 4096 UTF-8 bytes, and contain no NUL",
            });
        }
        Ok(Self {
            id,
            revision,
            instructions,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn revision(&self) -> u32 {
        self.revision
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// Bind the schema, identity, revision and original instruction bytes.
    pub fn digest(&self) -> Result<String, WorkPolicyError> {
        crate::content_digest::digest_json(WORK_POLICY_SCHEMA, &(WORK_POLICY_SCHEMA, self))
            .map_err(WorkPolicyError::Encoding)
    }
}

impl fmt::Debug for WorkPolicyV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkPolicyV1")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("instructions_bytes", &self.instructions.len())
            .finish()
    }
}

impl<'de> Deserialize<'de> for WorkPolicyV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            id: String,
            revision: u32,
            instructions: String,
        }

        let input = Input::deserialize(deserializer)?;
        Self::new(input.id, input.revision, input.instructions).map_err(serde::de::Error::custom)
    }
}

/// Stable refusals at policy boundaries; unknown wire values never default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkPolicyRefusal {
    Invalid,
    Conflict,
    Missing,
    Corrupt,
    LegacyRebind,
}

impl WorkPolicyRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "work_policy.invalid",
            Self::Conflict => "work_policy.conflict",
            Self::Missing => "work_policy.missing",
            Self::Corrupt => "work_policy.corrupt",
            Self::LegacyRebind => "work_policy.legacy_rebind",
        }
    }

    pub fn parse(value: &str) -> Result<Self, WorkPolicyRefusalParseError> {
        match value {
            "work_policy.invalid" => Ok(Self::Invalid),
            "work_policy.conflict" => Ok(Self::Conflict),
            "work_policy.missing" => Ok(Self::Missing),
            "work_policy.corrupt" => Ok(Self::Corrupt),
            "work_policy.legacy_rebind" => Ok(Self::LegacyRebind),
            _ => Err(WorkPolicyRefusalParseError(value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown WorkPolicyRefusal {0:?}; expected work_policy.invalid, work_policy.conflict, work_policy.missing, work_policy.corrupt or work_policy.legacy_rebind")]
pub struct WorkPolicyRefusalParseError(pub String);

#[derive(Debug, Error)]
pub enum WorkPolicyError {
    #[error("work_policy.invalid: {field} {reason}")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[error("work_policy.corrupt: policy digest encoding failed: {0}")]
    Encoding(#[source] serde_json::Error),
    #[error("work_policy.conflict: policy differs from this session's admitted value")]
    Conflict,
    #[error("work_policy.corrupt: {0}")]
    Corrupt(&'static str),
    #[error("work_policy.legacy_rebind: execution history cannot gain a new policy")]
    LegacyRebind,
    #[error("work_policy.corrupt: original log failed: {0}")]
    Log(#[source] a3s_effect::ActorError),
}

impl WorkPolicyError {
    pub fn refusal(&self) -> WorkPolicyRefusal {
        match self {
            Self::Invalid { .. } => WorkPolicyRefusal::Invalid,
            Self::Encoding(_) | Self::Corrupt(_) | Self::Log(_) => WorkPolicyRefusal::Corrupt,
            Self::Conflict => WorkPolicyRefusal::Conflict,
            Self::LegacyRebind => WorkPolicyRefusal::LegacyRebind,
        }
    }
}

mod admission;
pub(crate) use admission::{commit_policy, prepare_policy, PreparedPolicy, POLICY_KIND};

#[cfg(test)]
mod tests;
