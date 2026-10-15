//! Negative recovery provenance on the session's original authority log.
//!
//! This contract validates a host assertion and its content-bound identity.
//! It does not prove source evidence, admit execution, or clear any debt.

use a3s_effect::{Fact, NewFact};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use crate::session_checkpoint::{
    canonical_identity_digest, loop_checkpoint_content_identity, validate_checkpoint_id,
    SessionLogicalResumeEvidenceV1,
};

pub(super) const RECOVERY_UNPROVED_FACT_KIND: &str = "recovery.unproved";
const RECOVERY_UNPROVED_SCHEMA: &str = "a3s.code.recovery-unproved.v1";
const RECOVERY_UNPROVED_IDENTITY_DOMAIN: &str = "a3s.code.recovery-unproved.identity.v1";
const SOURCE_BINDING_DIGEST_DOMAIN: &str = "a3s.code.tool-admission-binding.v1";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(super) enum RecoveryUnprovedError {
    #[error("tool_effect_recovery_unproved_invalid: {0}")]
    Invalid(String),
    #[error("tool_effect_recovery_unproved_schema_unsupported: {0}")]
    SchemaUnsupported(String),
}

impl RecoveryUnprovedError {
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "tool_effect_recovery_unproved_invalid",
            Self::SchemaUnsupported(_) => "tool_effect_recovery_unproved_schema_unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryUnprovedReasonV1 {
    CacheEvidenceMissing,
}

impl RecoveryUnprovedReasonV1 {
    pub(super) fn parse(value: &str) -> Result<Self, RecoveryUnprovedError> {
        match value {
            "cache_evidence_missing" => Ok(Self::CacheEvidenceMissing),
            _ => Err(RecoveryUnprovedError::Invalid(format!(
                "RecoveryUnprovedReasonV1 refuses {value:?}"
            ))),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::CacheEvidenceMissing => "cache_evidence_missing",
        }
    }
}

impl Serialize for RecoveryUnprovedReasonV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RecoveryUnprovedReasonV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryUnprovedV1 {
    schema: String,
    session_id: String,
    thread_id: String,
    source_run_id: String,
    source_checkpoint_schema_version: u32,
    source_checkpoint_content_digest: String,
    // deserialize_with, without default, makes this nullable field required.
    #[serde(deserialize_with = "required_nullable_binding")]
    source_capability_binding_digest: Option<String>,
    reason: RecoveryUnprovedReasonV1,
}

fn required_nullable_binding<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

impl RecoveryUnprovedV1 {
    pub(super) fn from_checkpoint(
        checkpoint: &LoopCheckpoint,
        session_id: &str,
        thread_id: &str,
    ) -> Result<Self, RecoveryUnprovedError> {
        checkpoint
            .ensure_owned_by(&checkpoint.run_id, session_id)
            .map_err(invalid)?;
        validate_source_ownership(session_id, thread_id)?;
        let source_checkpoint_content_digest =
            if checkpoint.schema_version == LOOP_CHECKPOINT_SCHEMA_VERSION {
                let evidence =
                    SessionLogicalResumeEvidenceV1::from_checkpoint(checkpoint).map_err(invalid)?;
                evidence.validate_for(checkpoint).map_err(invalid)?;
                evidence.content_digest
            } else {
                loop_checkpoint_content_identity(checkpoint)
                    .map_err(invalid)?
                    .1
            };
        let source_capability_binding_digest = checkpoint
            .capability_binding
            .as_ref()
            .map(|binding| {
                crate::content_digest::digest_json(SOURCE_BINDING_DIGEST_DOMAIN, binding)
                    .map_err(invalid)
            })
            .transpose()?;
        let value = Self {
            schema: RECOVERY_UNPROVED_SCHEMA.into(),
            session_id: session_id.into(),
            thread_id: thread_id.into(),
            source_run_id: checkpoint.run_id.clone(),
            source_checkpoint_schema_version: checkpoint.schema_version,
            source_checkpoint_content_digest,
            source_capability_binding_digest,
            reason: RecoveryUnprovedReasonV1::CacheEvidenceMissing,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), RecoveryUnprovedError> {
        if self.schema != RECOVERY_UNPROVED_SCHEMA {
            return Err(RecoveryUnprovedError::SchemaUnsupported(
                self.schema.clone(),
            ));
        }
        validate_source_ownership(&self.session_id, &self.thread_id)?;
        validate_checkpoint_id("recovery-unproved source_run_id", &self.source_run_id)
            .map_err(invalid)?;
        if self.source_checkpoint_schema_version > LOOP_CHECKPOINT_SCHEMA_VERSION {
            return Err(RecoveryUnprovedError::Invalid(
                "source checkpoint schema is not loadable by this build".into(),
            ));
        }
        validate_digest(
            "source_checkpoint_content_digest",
            &self.source_checkpoint_content_digest,
        )?;
        if let Some(binding) = &self.source_capability_binding_digest {
            validate_digest("source_capability_binding_digest", binding)?;
        }
        Ok(())
    }

    pub(super) fn key(&self) -> Result<String, RecoveryUnprovedError> {
        self.validate()?;
        let digest =
            canonical_identity_digest(RECOVERY_UNPROVED_IDENTITY_DOMAIN, self).map_err(invalid)?;
        Ok(format!("recovery-unproved:{digest}"))
    }

    pub(super) fn into_fact(self) -> Result<NewFact, RecoveryUnprovedError> {
        let key = self.key()?;
        Ok(NewFact {
            kind: RECOVERY_UNPROVED_FACT_KIND.into(),
            key,
            payload: serde_json::to_value(self).map_err(invalid)?,
        })
    }

    /// Validate stored host assertion integrity only. The fold has no source
    /// checkpoint to re-prove, and this negative fact can only add debt.
    pub(super) fn validate_stored_fact(fact: &Fact) -> Result<Self, RecoveryUnprovedError> {
        if fact.kind != RECOVERY_UNPROVED_FACT_KIND {
            return Err(RecoveryUnprovedError::Invalid(
                "fact kind is not recovery.unproved".into(),
            ));
        }
        if fact.cause.is_some() {
            return Err(RecoveryUnprovedError::Invalid(
                "host recovery input cannot have a transition cause".into(),
            ));
        }
        let value: Self = serde_json::from_value(fact.payload.clone()).map_err(invalid)?;
        let expected_key = value.key()?;
        if fact.key != expected_key {
            return Err(RecoveryUnprovedError::Invalid(
                "fact key does not bind the exact recovery-unproved payload".into(),
            ));
        }
        Ok(value)
    }
}

fn validate_source_ownership(
    session_id: &str,
    thread_id: &str,
) -> Result<(), RecoveryUnprovedError> {
    validate_checkpoint_id("recovery-unproved session_id", session_id).map_err(invalid)?;
    validate_checkpoint_id("recovery-unproved thread_id", thread_id).map_err(invalid)?;
    if thread_id != super::thread_for_session(session_id) {
        return Err(RecoveryUnprovedError::Invalid(
            "thread_id does not match the exact source Session thread".into(),
        ));
    }
    Ok(())
}

fn validate_digest(field: &str, value: &str) -> Result<(), RecoveryUnprovedError> {
    crate::content_digest::validate_digest(value)
        .map_err(|_| RecoveryUnprovedError::Invalid(format!("{field} is not a canonical digest")))
}

fn invalid(error: impl std::fmt::Display) -> RecoveryUnprovedError {
    RecoveryUnprovedError::Invalid(error.to_string())
}

#[cfg(test)]
mod tests;
