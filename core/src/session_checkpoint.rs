//! Portable, content-addressed A3S Code session checkpoints.
//!
//! A checkpoint artifact binds one complete [`SessionSnapshotV1`] and,
//! optionally, the exact [`LoopCheckpoint`] from which Code can continue a
//! non-terminal run. The payload contains provider state only. A host may put
//! the canonical bytes in its authorized immutable-object store, while
//! checkpoint identity, retention, approval, and fork lineage remain outside
//! Code.

use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use crate::store::{SessionSnapshotV1, SESSION_SNAPSHOT_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use self::codec::{
    domain_digest, invalid_descriptor, validate_content_identity, validate_id, validate_sha256,
};

pub const SESSION_CHECKPOINT_DESCRIPTOR_SCHEMA_V1: &str =
    "a3s.code.session-checkpoint-descriptor.v1";
pub const SESSION_CHECKPOINT_PAYLOAD_SCHEMA_V1: &str = "a3s.code.session-checkpoint-payload.v1";
pub const SESSION_SNAPSHOT_EVIDENCE_SCHEMA_V1: &str = "a3s.code.session-snapshot-evidence.v1";
pub const SESSION_LOGICAL_RESUME_EVIDENCE_SCHEMA_V1: &str = "a3s.code.logical-resume-evidence.v1";
pub const SESSION_CHECKPOINT_FORMAT_V1: &str = "a3s_code_session_checkpoint_v1";
pub const SESSION_CHECKPOINT_MEDIA_TYPE_V1: &str =
    "application/vnd.a3s.code.session-checkpoint.v1+json";
pub const SESSION_CHECKPOINT_ENCODING_V1: &str = "canonical_json_v1";
pub const SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1: &str = "between_tool_rounds_v1";
pub const SESSION_CHECKPOINT_MAX_CONTENT_BYTES: u64 = 256 * 1024 * 1024;

const SNAPSHOT_EVIDENCE_DIGEST_DOMAIN_V1: &str = "a3s.code.session-snapshot-evidence-digest.v1";
const LOGICAL_RESUME_EVIDENCE_DIGEST_DOMAIN_V1: &str = "a3s.code.logical-resume-evidence-digest.v1";
const CHECKPOINT_DESCRIPTOR_DIGEST_DOMAIN_V1: &str =
    "a3s.code.session-checkpoint-descriptor-digest.v1";
pub(super) const MAX_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionCheckpointError {
    #[error("invalid session checkpoint descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("invalid session checkpoint payload: {0}")]
    InvalidPayload(String),
    #[error("session checkpoint content drift: {0}")]
    ContentDrift(String),
    #[error("session checkpoint encoding failed: {0}")]
    Encoding(String),
}

pub type SessionCheckpointResult<T> = std::result::Result<T, SessionCheckpointError>;

impl SessionCheckpointError {
    /// Stable machine-readable code for host and protocol adapters.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDescriptor(_) => "a3s.code.session_checkpoint.invalid_descriptor",
            Self::InvalidPayload(_) => "a3s.code.session_checkpoint.invalid_payload",
            Self::ContentDrift(_) => "a3s.code.session_checkpoint.content_drift",
            Self::Encoding(_) => "a3s.code.session_checkpoint.encoding",
        }
    }
}

/// Exact content identity of the aggregate Session snapshot component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSnapshotEvidenceV1 {
    pub schema: String,
    pub encoding: String,
    pub session_id: String,
    pub snapshot_schema_version: u32,
    pub size_bytes: u64,
    pub content_digest: String,
    pub evidence_digest: String,
}

impl SessionSnapshotEvidenceV1 {
    pub fn from_snapshot(snapshot: &SessionSnapshotV1) -> SessionCheckpointResult<Self> {
        codec::snapshot_evidence(snapshot)
    }

    pub fn validate(&self) -> SessionCheckpointResult<()> {
        if self.schema != SESSION_SNAPSHOT_EVIDENCE_SCHEMA_V1 {
            return Err(invalid_descriptor(
                "snapshot evidence schema is unsupported",
            ));
        }
        if self.encoding != SESSION_CHECKPOINT_ENCODING_V1 {
            return Err(invalid_descriptor("snapshot encoding is unsupported"));
        }
        validate_id("snapshot session_id", &self.session_id)?;
        if self.snapshot_schema_version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            return Err(invalid_descriptor(
                "snapshot schema version is not the exact portable v1 version",
            ));
        }
        validate_content_identity(self.size_bytes, &self.content_digest)?;
        validate_sha256("snapshot evidence_digest", &self.evidence_digest)?;
        if self.evidence_digest != self.expected_digest()? {
            return Err(invalid_descriptor(
                "snapshot evidence digest does not bind the exact snapshot identity",
            ));
        }
        Ok(())
    }

    pub fn validate_for(&self, snapshot: &SessionSnapshotV1) -> SessionCheckpointResult<()> {
        self.validate()?;
        if self != &Self::from_snapshot(snapshot)? {
            return Err(SessionCheckpointError::ContentDrift(
                "snapshot evidence does not match the exact supplied Session snapshot".into(),
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> SessionCheckpointResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            encoding: &'a str,
            session_id: &'a str,
            snapshot_schema_version: u32,
            size_bytes: u64,
            content_digest: &'a str,
        }

        domain_digest(
            SNAPSHOT_EVIDENCE_DIGEST_DOMAIN_V1,
            &DigestInput {
                schema: &self.schema,
                encoding: &self.encoding,
                session_id: &self.session_id,
                snapshot_schema_version: self.snapshot_schema_version,
                size_bytes: self.size_bytes,
                content_digest: &self.content_digest,
            },
        )
    }
}

/// Exact, content-free evidence for Code's tool-round resume boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLogicalResumeEvidenceV1 {
    pub schema: String,
    pub resume_semantics: String,
    pub session_id: String,
    pub source_run_id: String,
    pub checkpoint_schema_version: u32,
    pub completed_tool_rounds: u64,
    pub checkpoint_ms: u64,
    pub size_bytes: u64,
    pub content_digest: String,
    pub evidence_digest: String,
}

impl SessionLogicalResumeEvidenceV1 {
    pub fn from_checkpoint(checkpoint: &LoopCheckpoint) -> SessionCheckpointResult<Self> {
        codec::logical_resume_evidence(checkpoint)
    }

    pub fn validate(&self) -> SessionCheckpointResult<()> {
        if self.schema != SESSION_LOGICAL_RESUME_EVIDENCE_SCHEMA_V1 {
            return Err(invalid_descriptor(
                "logical-resume evidence schema is unsupported",
            ));
        }
        if self.resume_semantics != SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1 {
            return Err(invalid_descriptor(
                "logical-resume semantics are unsupported",
            ));
        }
        validate_id("logical-resume session_id", &self.session_id)?;
        validate_id("logical-resume source_run_id", &self.source_run_id)?;
        if self.checkpoint_schema_version != LOOP_CHECKPOINT_SCHEMA_VERSION {
            return Err(invalid_descriptor(
                "loop checkpoint schema version is not the exact portable v1 version",
            ));
        }
        if self.completed_tool_rounds == 0 {
            return Err(invalid_descriptor(
                "logical resume requires a completed tool-round boundary",
            ));
        }
        validate_content_identity(self.size_bytes, &self.content_digest)?;
        validate_sha256("logical-resume evidence_digest", &self.evidence_digest)?;
        if self.evidence_digest != self.expected_digest()? {
            return Err(invalid_descriptor(
                "logical-resume evidence digest does not bind the exact boundary",
            ));
        }
        Ok(())
    }

    pub fn validate_for(&self, checkpoint: &LoopCheckpoint) -> SessionCheckpointResult<()> {
        self.validate()?;
        if self != &Self::from_checkpoint(checkpoint)? {
            return Err(SessionCheckpointError::ContentDrift(
                "logical-resume evidence does not match the exact supplied loop checkpoint".into(),
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> SessionCheckpointResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            resume_semantics: &'a str,
            session_id: &'a str,
            source_run_id: &'a str,
            checkpoint_schema_version: u32,
            completed_tool_rounds: u64,
            checkpoint_ms: u64,
            size_bytes: u64,
            content_digest: &'a str,
        }

        domain_digest(
            LOGICAL_RESUME_EVIDENCE_DIGEST_DOMAIN_V1,
            &DigestInput {
                schema: &self.schema,
                resume_semantics: &self.resume_semantics,
                session_id: &self.session_id,
                source_run_id: &self.source_run_id,
                checkpoint_schema_version: self.checkpoint_schema_version,
                completed_tool_rounds: self.completed_tool_rounds,
                checkpoint_ms: self.checkpoint_ms,
                size_bytes: self.size_bytes,
                content_digest: &self.content_digest,
            },
        )
    }
}

/// Secret-free descriptor a host can store beside its own checkpoint record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheckpointDescriptorV1 {
    pub schema: String,
    pub format: String,
    pub media_type: String,
    pub snapshot: SessionSnapshotEvidenceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_resume: Option<SessionLogicalResumeEvidenceV1>,
    pub size_bytes: u64,
    pub content_digest: String,
    pub descriptor_digest: String,
}

impl SessionCheckpointDescriptorV1 {
    pub fn validate(&self) -> SessionCheckpointResult<()> {
        if self.schema != SESSION_CHECKPOINT_DESCRIPTOR_SCHEMA_V1 {
            return Err(invalid_descriptor(
                "checkpoint descriptor schema is unsupported",
            ));
        }
        if self.format != SESSION_CHECKPOINT_FORMAT_V1 {
            return Err(invalid_descriptor("checkpoint format is unsupported"));
        }
        if self.media_type != SESSION_CHECKPOINT_MEDIA_TYPE_V1 {
            return Err(invalid_descriptor("checkpoint media type is unsupported"));
        }
        self.snapshot.validate()?;
        if let Some(logical_resume) = &self.logical_resume {
            logical_resume.validate()?;
            if logical_resume.session_id != self.snapshot.session_id {
                return Err(invalid_descriptor(
                    "logical-resume and snapshot session identities differ",
                ));
            }
        }
        validate_content_identity(self.size_bytes, &self.content_digest)?;
        if self.snapshot.size_bytes > self.size_bytes
            || self
                .logical_resume
                .as_ref()
                .is_some_and(|evidence| evidence.size_bytes > self.size_bytes)
        {
            return Err(invalid_descriptor(
                "checkpoint component is larger than its containing payload",
            ));
        }
        validate_sha256("checkpoint descriptor_digest", &self.descriptor_digest)?;
        if self.descriptor_digest != self.expected_digest()? {
            return Err(invalid_descriptor(
                "descriptor digest does not bind the exact checkpoint components",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> SessionCheckpointResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            format: &'a str,
            media_type: &'a str,
            snapshot: &'a SessionSnapshotEvidenceV1,
            logical_resume: &'a Option<SessionLogicalResumeEvidenceV1>,
            size_bytes: u64,
            content_digest: &'a str,
        }

        domain_digest(
            CHECKPOINT_DESCRIPTOR_DIGEST_DOMAIN_V1,
            &DigestInput {
                schema: &self.schema,
                format: &self.format,
                media_type: &self.media_type,
                snapshot: &self.snapshot,
                logical_resume: &self.logical_resume,
                size_bytes: self.size_bytes,
                content_digest: &self.content_digest,
            },
        )
    }
}

/// Canonical provider payload stored behind the descriptor's content digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheckpointPayloadV1 {
    pub schema: String,
    pub snapshot: SessionSnapshotV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_resume: Option<LoopCheckpoint>,
}

impl SessionCheckpointPayloadV1 {
    pub fn into_parts(self) -> (SessionSnapshotV1, Option<LoopCheckpoint>) {
        (self.snapshot, self.logical_resume)
    }
}

mod artifact;
mod codec;

pub use artifact::SessionCheckpointExportV1;

/// Host-owned destination for exact checkpoints captured from a live Run.
///
/// Code invokes the sink only at a completed tool-round boundary, after the
/// Run's preceding events have been materialized into the same semantic
/// snapshot. The supplied export is already canonical, bounded, and fully
/// validated. It can contain conversation and Tool data, so authorization,
/// encryption, immutable-object storage, retention, and replication remain
/// host responsibilities.
///
/// Sink failures are logged and do not halt the live Run. Implementations
/// should therefore be idempotent by descriptor identity and return only after
/// the export has reached the durability level required by the host.
#[async_trait::async_trait]
pub trait SessionCheckpointExportSink: Send + Sync {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()>;
}
