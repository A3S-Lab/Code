//! Bounded evidence reads over the existing Code run and artifact stores.

use super::identity::{
    digest_bytes, digest_json, validate_digest, ExecutionFrameV1, ExecutionTargetV1,
};
use super::journal::{ExecutionFactInputV1, ExecutionFactJournal, ExecutionFactV1};
use crate::agent::AgentEvent;
use crate::event_protocol::{run_event_envelope_v1, EventEnvelopeV1};
use crate::run::{InMemoryRunStore, RunSnapshot, RunStatus};
use crate::tools::ArtifactStore;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use thiserror::Error;

pub const EVIDENCE_SNAPSHOT_SCHEMA_V1: &str = "a3s.code.evidence-snapshot.v1";
pub const EVIDENCE_MAX_EVENTS: usize = 4096;
pub const EVIDENCE_MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;
pub const EVIDENCE_MAX_ARTIFACTS: usize = 256;
pub const EVIDENCE_MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub const EVIDENCE_MAX_PROMPT_BYTES: usize = 1024 * 1024;
pub const EVIDENCE_MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_EVENT_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REFERENCED_ARTIFACTS: usize = 256;
const MAX_ARTIFACT_URI_BYTES: usize = 1024;
const MAX_TOOL_NAME_BYTES: usize = 256;
const MAX_EVENT_TYPE_BYTES: usize = 256;
const MAX_EVENT_METADATA_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceContentModeV1 {
    /// Return event payloads as digest/size markers only.
    #[default]
    DigestOnly,
    /// Return event payloads only while each encoded event remains within the
    /// request's byte budget; oversized payloads become digest markers.
    BoundedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLimitsV1 {
    pub max_events: usize,
    pub max_event_bytes: usize,
    pub max_artifacts: usize,
    pub max_artifact_bytes: usize,
    pub max_prompt_bytes: usize,
    pub max_result_bytes: usize,
}

impl Default for EvidenceLimitsV1 {
    fn default() -> Self {
        Self {
            max_events: 128,
            max_event_bytes: 256 * 1024,
            max_artifacts: 32,
            max_artifact_bytes: 1024 * 1024,
            max_prompt_bytes: 16 * 1024,
            max_result_bytes: 64 * 1024,
        }
    }
}

impl EvidenceLimitsV1 {
    pub fn validate(&self) -> Result<(), EvidenceError> {
        if self.max_events == 0
            || self.max_event_bytes == 0
            || self.max_artifact_bytes == 0
            || self.max_prompt_bytes == 0
            || self.max_result_bytes == 0
            || self.max_events > EVIDENCE_MAX_EVENTS
            || self.max_event_bytes > EVIDENCE_MAX_EVENT_BYTES
            || self.max_artifacts > EVIDENCE_MAX_ARTIFACTS
            || self.max_artifact_bytes > EVIDENCE_MAX_ARTIFACT_BYTES
            || self.max_prompt_bytes > EVIDENCE_MAX_PROMPT_BYTES
            || self.max_result_bytes > EVIDENCE_MAX_RESULT_BYTES
        {
            return Err(EvidenceError::InvalidLimit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReadRequestV1 {
    pub target: ExecutionTargetV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub limits: EvidenceLimitsV1,
    #[serde(default)]
    pub content_mode: EvidenceContentModeV1,
    #[serde(default)]
    pub include_prompt: bool,
    /// Include bounded terminal result/error text in addition to their
    /// digests. The default remains digest-only for cross-tenant safety.
    #[serde(default)]
    pub include_terminal_text: bool,
    #[serde(default)]
    pub include_artifact_content: bool,
}

impl EvidenceReadRequestV1 {
    pub fn new(target: ExecutionTargetV1) -> Self {
        Self {
            target,
            after_sequence: None,
            limits: EvidenceLimitsV1::default(),
            content_mode: EvidenceContentModeV1::DigestOnly,
            include_prompt: false,
            include_terminal_text: false,
            include_artifact_content: false,
        }
    }

    pub fn validate(&self) -> Result<(), EvidenceError> {
        self.target
            .validate()
            .map_err(|_| EvidenceError::InvalidTarget)?;
        self.limits.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRunStateV1 {
    pub schema: String,
    pub target: ExecutionTargetV1,
    pub status: RunStatus,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub event_count: u64,
    pub prompt_bytes: u64,
    pub prompt_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_change_set_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_binding_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cognitive_binding_digest: Option<String>,
}

impl EvidenceRunStateV1 {
    fn from_snapshot(
        snapshot: &RunSnapshot,
        target: &ExecutionTargetV1,
        limits: EvidenceLimitsV1,
        include_prompt: bool,
        include_terminal_text: bool,
    ) -> Result<Self, EvidenceError> {
        let prompt = if include_prompt {
            bounded_string(&snapshot.prompt, limits.max_prompt_bytes)
        } else {
            None
        };
        let prompt_bytes =
            u64::try_from(snapshot.prompt.len()).map_err(|_| EvidenceError::NumericOverflow)?;
        let prompt_digest = digest_bytes("a3s.code.evidence.prompt.v1", snapshot.prompt.as_bytes());
        let result_bytes = snapshot
            .result_text
            .as_ref()
            .map(|value| u64::try_from(value.len()))
            .transpose()
            .map_err(|_| EvidenceError::NumericOverflow)?;
        let result_digest = snapshot
            .result_text
            .as_deref()
            .map(|value| digest_bytes("a3s.code.evidence.result.v1", value.as_bytes()));
        let error_bytes = snapshot
            .error
            .as_ref()
            .map(|value| u64::try_from(value.len()))
            .transpose()
            .map_err(|_| EvidenceError::NumericOverflow)?;
        let error_digest = snapshot
            .error
            .as_deref()
            .map(|value| digest_bytes("a3s.code.evidence.error.v1", value.as_bytes()));
        let result = if include_terminal_text {
            snapshot
                .result_text
                .as_deref()
                .and_then(|value| bounded_string(value, limits.max_result_bytes))
        } else {
            None
        };
        let error = if include_terminal_text {
            snapshot
                .error
                .as_deref()
                .and_then(|value| bounded_string(value, limits.max_result_bytes))
        } else {
            None
        };
        let workspace_change_set_digest = snapshot
            .workspace_change_set
            .as_ref()
            .map(|value| digest_json("a3s.code.evidence.workspace-change-set.v1", value))
            .transpose()
            .map_err(EvidenceError::Serialization)?;
        let capability_binding_digest = snapshot
            .capability_binding
            .as_ref()
            .map(|value| digest_json("a3s.code.evidence.capability-binding.v1", value))
            .transpose()
            .map_err(EvidenceError::Serialization)?;
        let cognitive_binding_digest = snapshot
            .cognitive_package_binding
            .as_ref()
            .map(|value| digest_json("a3s.code.evidence.cognitive-binding.v1", value))
            .transpose()
            .map_err(EvidenceError::Serialization)?;
        Ok(Self {
            schema: "a3s.code.evidence-run-state.v1".to_string(),
            target: target.clone(),
            status: snapshot.status,
            created_at_ms: snapshot.created_at_ms,
            updated_at_ms: snapshot.updated_at_ms,
            event_count: u64::try_from(snapshot.event_count)
                .map_err(|_| EvidenceError::NumericOverflow)?,
            prompt_bytes,
            prompt_digest,
            prompt,
            result_bytes,
            result,
            error_bytes,
            error,
            result_digest,
            error_digest,
            workspace_change_set_digest,
            capability_binding_digest,
            cognitive_binding_digest,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEventV1 {
    pub sequence: u64,
    pub occurred_at_ms: u64,
    pub event: EventEnvelopeV1,
    pub payload_digest: String,
    pub payload_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArtifactV1 {
    pub artifact_uri: String,
    pub tool_name: String,
    pub content_digest: String,
    pub content_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSnapshotV1 {
    pub schema: String,
    pub target: ExecutionTargetV1,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    pub first_available_sequence: Option<u64>,
    pub latest_sequence_exclusive: u64,
    pub state: EvidenceRunStateV1,
    pub facts: Vec<ExecutionFactV1>,
    pub events: Vec<EvidenceEventV1>,
    pub artifacts: Vec<EvidenceArtifactV1>,
    pub complete: bool,
    pub retention_gap: bool,
    pub snapshot_digest: String,
}

impl EvidenceSnapshotV1 {
    pub fn validate(&self) -> Result<(), EvidenceError> {
        if self.schema != EVIDENCE_SNAPSHOT_SCHEMA_V1 {
            return Err(EvidenceError::UnsupportedSchema);
        }
        self.target
            .validate()
            .map_err(|_| EvidenceError::InvalidTarget)?;
        if self.state.target != self.target {
            return Err(EvidenceError::TargetMismatch);
        }
        if self
            .first_available_sequence
            .is_some_and(|first| first >= self.latest_sequence_exclusive)
        {
            return Err(EvidenceError::InvalidField("first_available_sequence"));
        }
        if self.state.schema != "a3s.code.evidence-run-state.v1" {
            return Err(EvidenceError::UnsupportedSchema);
        }
        if self.state.event_count != self.latest_sequence_exclusive {
            return Err(EvidenceError::InvalidField("state.event_count"));
        }
        if self.facts.len() > EVIDENCE_MAX_EVENTS
            || self.events.len() > EVIDENCE_MAX_EVENTS
            || self.artifacts.len() > EVIDENCE_MAX_ARTIFACTS
        {
            return Err(EvidenceError::InvalidLimit);
        }
        if self.state.created_at_ms > self.state.updated_at_ms
            || self.observed_at_ms < self.state.updated_at_ms
        {
            return Err(EvidenceError::InvalidField("state.timestamps"));
        }
        let requested_start = self
            .after_sequence
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        let expected_gap = requested_start < self.latest_sequence_exclusive
            && self
                .first_available_sequence
                .is_none_or(|first| requested_start < first);
        if self.retention_gap != expected_gap {
            return Err(EvidenceError::InvalidField("retention_gap"));
        }
        validate_digest(&self.state.prompt_digest)
            .map_err(|_| EvidenceError::InvalidDigest("state.prompt_digest"))?;
        if let Some(prompt) = self.state.prompt.as_deref() {
            let prompt_bytes =
                u64::try_from(prompt.len()).map_err(|_| EvidenceError::NumericOverflow)?;
            if self.state.prompt_bytes != prompt_bytes {
                return Err(EvidenceError::InvalidField("state.prompt_bytes"));
            }
            if self.state.prompt_digest
                != digest_bytes("a3s.code.evidence.prompt.v1", prompt.as_bytes())
            {
                return Err(EvidenceError::DigestMismatch("state.prompt_digest"));
            }
        }
        if self.state.result.is_some() && self.state.result_digest.is_none() {
            return Err(EvidenceError::InvalidField("state.result_digest"));
        }
        if self.state.error.is_some() && self.state.error_digest.is_none() {
            return Err(EvidenceError::InvalidField("state.error_digest"));
        }
        if self.state.result_bytes.is_some() != self.state.result_digest.is_some()
            || self.state.error_bytes.is_some() != self.state.error_digest.is_some()
        {
            return Err(EvidenceError::InvalidField("state.text_bytes"));
        }
        for (field, digest) in [
            ("state.result_digest", self.state.result_digest.as_deref()),
            ("state.error_digest", self.state.error_digest.as_deref()),
        ] {
            if let Some(digest) = digest {
                validate_digest(digest).map_err(|_| EvidenceError::InvalidDigest(field))?;
            }
        }
        if let (Some(result), Some(digest)) = (
            self.state.result.as_deref(),
            self.state.result_digest.as_deref(),
        ) {
            if self.state.result_bytes != u64::try_from(result.len()).ok() {
                return Err(EvidenceError::InvalidField("state.result_bytes"));
            }
            if digest != digest_bytes("a3s.code.evidence.result.v1", result.as_bytes()) {
                return Err(EvidenceError::DigestMismatch("state.result_digest"));
            }
        }
        if let (Some(error), Some(digest)) = (
            self.state.error.as_deref(),
            self.state.error_digest.as_deref(),
        ) {
            if self.state.error_bytes != u64::try_from(error.len()).ok() {
                return Err(EvidenceError::InvalidField("state.error_bytes"));
            }
            if digest != digest_bytes("a3s.code.evidence.error.v1", error.as_bytes()) {
                return Err(EvidenceError::DigestMismatch("state.error_digest"));
            }
        }
        let mut previous_fact: Option<(u64, u64)> = None;
        for fact in &self.facts {
            fact.validate().map_err(EvidenceError::Journal)?;
            if fact.frame.target != self.target {
                return Err(EvidenceError::TargetMismatch);
            }
            if fact.observed_at_ms > self.observed_at_ms {
                return Err(EvidenceError::InvalidField("facts.observed_at_ms"));
            }
            if self
                .after_sequence
                .is_some_and(|cursor| fact.sequence <= cursor)
                || fact.sequence >= self.latest_sequence_exclusive
                || previous_fact.is_some_and(|(sequence, timestamp)| {
                    fact.sequence != sequence.saturating_add(1) || fact.observed_at_ms < timestamp
                })
            {
                return Err(EvidenceError::InvalidField("facts.sequence"));
            }
            previous_fact = Some((fact.sequence, fact.observed_at_ms));
        }
        let mut previous_event: Option<(u64, u64)> = None;
        for (index, event) in self.events.iter().enumerate() {
            validate_digest(&event.payload_digest)
                .map_err(|_| EvidenceError::InvalidDigest("payload_digest"))?;
            let payload = serde_json::to_vec(&event.event).map_err(EvidenceError::Serialization)?;
            if event.payload_bytes == 0 || event.payload_bytes > MAX_EVENT_PAYLOAD_BYTES {
                return Err(EvidenceError::InvalidField("payload_bytes"));
            }
            if event.event.version != 1
                || event.event.event_type.is_empty()
                || event.event.event_type.len() > MAX_EVENT_TYPE_BYTES
                || event.event.event_type.contains('\0')
                || event.event.event_type.lines().count() != 1
                || payload.is_empty()
            {
                return Err(EvidenceError::InvalidField("event"));
            }
            validate_event_metadata(
                &event.event,
                &self.target,
                event.sequence,
                event.occurred_at_ms,
            )?;
            if event.occurred_at_ms > self.observed_at_ms {
                return Err(EvidenceError::InvalidField("events.occurred_at_ms"));
            }
            if self
                .after_sequence
                .is_some_and(|cursor| event.sequence <= cursor)
                || event.sequence >= self.latest_sequence_exclusive
                || previous_event.is_some_and(|(sequence, timestamp)| {
                    event.sequence != sequence.saturating_add(1) || event.occurred_at_ms < timestamp
                })
            {
                return Err(EvidenceError::InvalidField("events.sequence"));
            }
            let redacted = is_redacted_payload(
                &event.event.payload,
                &event.payload_digest,
                event.payload_bytes,
            );
            if redacted {
                let marker = event
                    .event
                    .payload
                    .as_object()
                    .ok_or(EvidenceError::InvalidField("event.payload"))?;
                let marker_digest = marker
                    .get("digest")
                    .and_then(Value::as_str)
                    .ok_or(EvidenceError::InvalidField("event.payload"))?;
                let marker_bytes = marker
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .ok_or(EvidenceError::InvalidField("event.payload"))?;
                if marker_digest != event.payload_digest || marker_bytes != event.payload_bytes {
                    return Err(EvidenceError::DigestMismatch("payload_digest"));
                }
            } else if event.payload_bytes != u64::try_from(payload.len()).unwrap_or(u64::MAX)
                || event.payload_digest
                    != digest_bytes("a3s.code.evidence.event-payload.v1", &payload)
            {
                return Err(EvidenceError::DigestMismatch("payload_digest"));
            }
            if let Some(fact) = self.facts.get(index) {
                // A fact and an event at the same cursor must describe the
                // same source observation. A snapshot may be marked
                // incomplete while two independently captured stores are
                // converging, but it must never claim that mismatched data is
                // a complete evidence window.
                let pair_matches = fact.sequence == event.sequence
                    && fact.event_type == event.event.event_type
                    && fact.observed_at_ms == event.occurred_at_ms
                    && fact_payload_matches_event(fact, event);
                if !pair_matches && self.complete {
                    return Err(EvidenceError::InvalidField("facts.events"));
                }
            }
            previous_event = Some((event.sequence, event.occurred_at_ms));
        }
        if self.complete {
            let event_sequences = self
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>();
            let fact_sequences = self
                .facts
                .iter()
                .map(|fact| fact.sequence)
                .collect::<Vec<_>>();
            if event_sequences != fact_sequences {
                return Err(EvidenceError::InvalidField("facts.events"));
            }
        }
        for artifact in &self.artifacts {
            if artifact.artifact_uri.is_empty()
                || artifact.artifact_uri.len() > MAX_ARTIFACT_URI_BYTES
                || artifact.artifact_uri.contains('\0')
                || artifact.artifact_uri.lines().count() != 1
                || artifact.tool_name.is_empty()
                || artifact.tool_name.len() > MAX_TOOL_NAME_BYTES
                || artifact.tool_name.contains('\0')
                || artifact.tool_name.lines().count() != 1
            {
                return Err(EvidenceError::InvalidField("artifact"));
            }
            validate_digest(&artifact.content_digest)
                .map_err(|_| EvidenceError::InvalidDigest("content_digest"))?;
            if artifact.content.as_ref().is_some_and(|content| {
                u64::try_from(content.len()).ok() != Some(artifact.content_bytes)
            }) {
                return Err(EvidenceError::InvalidField("artifact.content"));
            }
            if let Some(content) = artifact.content.as_deref() {
                if artifact.content_digest
                    != digest_bytes("a3s.code.evidence.artifact-content.v1", content.as_bytes())
                {
                    return Err(EvidenceError::DigestMismatch("artifact.content_digest"));
                }
            }
        }
        if self
            .artifacts
            .windows(2)
            .any(|window| window[0].artifact_uri >= window[1].artifact_uri)
        {
            return Err(EvidenceError::InvalidField("artifacts"));
        }
        if self.retention_gap && self.complete {
            return Err(EvidenceError::InvalidField("complete"));
        }
        if let Some(first) = self.events.first() {
            if (self.retention_gap && self.first_available_sequence != Some(first.sequence))
                || (!self.retention_gap
                    && requested_start < self.latest_sequence_exclusive
                    && first.sequence != requested_start)
                || self
                    .first_available_sequence
                    .is_some_and(|available| first.sequence < available)
            {
                return Err(EvidenceError::InvalidField("events"));
            }
        } else if self.retention_gap && self.first_available_sequence.is_some() {
            return Err(EvidenceError::InvalidField("events"));
        }
        let last_sequence = self.events.last().map(|event| event.sequence);
        if requested_start < self.latest_sequence_exclusive {
            let page_complete = last_sequence.is_some_and(|sequence| {
                sequence.saturating_add(1) == self.latest_sequence_exclusive
            });
            if self.complete && !page_complete {
                return Err(EvidenceError::InvalidField("complete"));
            }
            if self.events.is_empty() && self.complete {
                return Err(EvidenceError::InvalidField("complete"));
            }
        }
        if self.snapshot_digest != self.expected_digest()? {
            return Err(EvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, EvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            target: &'a ExecutionTargetV1,
            observed_at_ms: u64,
            after_sequence: Option<u64>,
            first_available_sequence: Option<u64>,
            latest_sequence_exclusive: u64,
            state: &'a EvidenceRunStateV1,
            facts: &'a [ExecutionFactV1],
            events: &'a [EvidenceEventV1],
            artifacts: &'a [EvidenceArtifactV1],
            complete: bool,
            retention_gap: bool,
        }
        digest_json(
            "a3s.code.evidence-snapshot.identity.v1",
            &Identity {
                schema: &self.schema,
                target: &self.target,
                observed_at_ms: self.observed_at_ms,
                after_sequence: self.after_sequence,
                first_available_sequence: self.first_available_sequence,
                latest_sequence_exclusive: self.latest_sequence_exclusive,
                state: &self.state,
                facts: &self.facts,
                events: &self.events,
                artifacts: &self.artifacts,
                complete: self.complete,
                retention_gap: self.retention_gap,
            },
        )
        .map_err(EvidenceError::Serialization)
    }
}

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("evidence target is invalid or unknown")]
    InvalidTarget,
    #[error("evidence target does not match its source")]
    TargetMismatch,
    #[error("evidence schema is unsupported")]
    UnsupportedSchema,
    #[error("evidence field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("evidence digest for `{0}` is invalid")]
    InvalidDigest(&'static str),
    #[error("evidence digest for `{0}` does not match")]
    DigestMismatch(&'static str),
    #[error("evidence limit is invalid")]
    InvalidLimit,
    #[error("evidence numeric value does not fit the wire type")]
    NumericOverflow,
    #[error("run was not found")]
    RunNotFound,
    #[error("evidence serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("execution fact error: {0}")]
    Journal(#[from] super::journal::JournalError),
}

#[async_trait]
pub trait EvidenceReader: Send + Sync {
    async fn read(
        &self,
        request: EvidenceReadRequestV1,
    ) -> Result<EvidenceSnapshotV1, EvidenceError>;
}

/// Reader over the native in-memory run journal and optional artifact/fact
/// stores.  It captures state and the event window from one RunStore
/// observation generation.
#[derive(Clone)]
pub struct RunEvidenceReader {
    runs: Arc<InMemoryRunStore>,
    facts: Option<Arc<dyn ExecutionFactJournal>>,
    artifacts: Option<ArtifactStore>,
}

impl std::fmt::Debug for RunEvidenceReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunEvidenceReader")
            .field("facts_bound", &self.facts.is_some())
            .field("artifacts_bound", &self.artifacts.is_some())
            .finish()
    }
}

impl RunEvidenceReader {
    pub fn new(runs: Arc<InMemoryRunStore>) -> Self {
        Self {
            runs,
            facts: None,
            artifacts: None,
        }
    }

    pub fn with_facts(mut self, facts: Arc<dyn ExecutionFactJournal>) -> Self {
        self.facts = Some(facts);
        self
    }

    pub fn with_artifacts(mut self, artifacts: ArtifactStore) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    pub async fn read(
        &self,
        request: EvidenceReadRequestV1,
    ) -> Result<EvidenceSnapshotV1, EvidenceError> {
        self.read_inner(request).await
    }

    async fn read_inner(
        &self,
        request: EvidenceReadRequestV1,
    ) -> Result<EvidenceSnapshotV1, EvidenceError> {
        request.validate()?;
        let after_sequence = request
            .after_sequence
            .map(|sequence| usize::try_from(sequence).map_err(|_| EvidenceError::NumericOverflow))
            .transpose()?;
        let observation = self
            .runs
            .event_observation(
                &request.target.run_id,
                after_sequence,
                request.limits.max_events,
            )
            .await
            .ok_or(EvidenceError::RunNotFound)?;
        if observation.snapshot.session_id != request.target.session_id {
            return Err(EvidenceError::TargetMismatch);
        }

        let state_content_truncated = state_content_truncated(
            &observation.snapshot,
            request.limits,
            request.include_prompt,
            request.include_terminal_text,
        );
        let state = EvidenceRunStateV1::from_snapshot(
            &observation.snapshot,
            &request.target,
            request.limits,
            request.include_prompt,
            request.include_terminal_text,
        )?;
        let frame = ExecutionFrameV1::root(request.target.clone());
        let observation_first_available = observation
            .page
            .first_available_sequence
            .map(|sequence| u64::try_from(sequence).map_err(|_| EvidenceError::NumericOverflow))
            .transpose()?;
        let observation_latest_sequence_exclusive =
            u64::try_from(observation.page.latest_sequence_exclusive)
                .map_err(|_| EvidenceError::NumericOverflow)?;
        let mut events = Vec::new();
        let mut event_bytes = 0usize;
        // A bounded page with more source events is not a complete evidence
        // window, even when no retention gap exists. Callers can request the
        // next page with the last returned sequence.
        let mut complete = !observation.page.retention_gap
            && !observation.page.has_more
            && !state_content_truncated;
        let mut referenced_artifacts = BTreeSet::new();
        let mut references_truncated = false;

        for record in &observation.page.events {
            let sequence =
                u64::try_from(record.sequence).map_err(|_| EvidenceError::NumericOverflow)?;
            let envelope =
                run_event_envelope_v1(record, &request.target.run_id, &request.target.session_id)
                    .map_err(|_| EvidenceError::InvalidField("event"))?;
            let encoded = serde_json::to_vec(&envelope).map_err(EvidenceError::Serialization)?;
            let payload_digest = digest_bytes("a3s.code.evidence.event-payload.v1", &encoded);
            let payload_bytes =
                u64::try_from(encoded.len()).map_err(|_| EvidenceError::NumericOverflow)?;
            let mut projected = envelope.clone();
            let over_budget = encoded.len() > request.limits.max_event_bytes
                || event_bytes.saturating_add(encoded.len()) > request.limits.max_event_bytes;
            if request.content_mode == EvidenceContentModeV1::DigestOnly || over_budget {
                projected.payload = serde_json::json!({
                    "digest": payload_digest,
                    "bytes": payload_bytes,
                    "content": "redacted"
                });
                // Digest-only is an intentional, complete representation of
                // the event window.  Only an omitted payload caused by a
                // bounded-payload budget makes the source incomplete.
                if request.content_mode == EvidenceContentModeV1::BoundedPayload {
                    complete &= !over_budget;
                }
            } else {
                event_bytes = event_bytes.saturating_add(encoded.len());
            }
            references_truncated |=
                collect_refs_from_value(&envelope.payload, &mut referenced_artifacts);
            events.push(EvidenceEventV1 {
                sequence,
                occurred_at_ms: record.timestamp_ms,
                event: projected,
                payload_digest,
                payload_bytes,
            });
        }

        let facts = if let Some(journal) = &self.facts {
            let page = journal
                .page(
                    &request.target,
                    request.after_sequence,
                    request.limits.max_events,
                )
                .ok_or(EvidenceError::RunNotFound)?;
            complete &= !page.retention_gap && !page.has_more;
            if page.latest_sequence_exclusive != observation_latest_sequence_exclusive {
                complete = false;
            }
            page.facts
        } else {
            observation
                .page
                .events
                .iter()
                .map(|record| {
                    let input = ExecutionFactInputV1::from_run_event(frame.clone(), record)?;
                    ExecutionFactV1::from_input(input).map_err(EvidenceError::Journal)
                })
                .collect::<Result<Vec<_>, EvidenceError>>()?
        };
        let event_sequences = events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        let fact_sequences = facts.iter().map(|fact| fact.sequence).collect::<Vec<_>>();
        if event_sequences != fact_sequences {
            // The run store and an independently durable fact journal do not
            // share a transaction. Never claim a complete evidence window
            // when their generations disagree.
            complete = false;
        }
        if events.len() != facts.len()
            || events.iter().zip(&facts).any(|(event, fact)| {
                event.sequence != fact.sequence
                    || event.event.event_type != fact.event_type
                    || event.occurred_at_ms != fact.observed_at_ms
                    || !fact_payload_matches_event(fact, event)
            })
        {
            // The fact journal and the run store are separate observations.
            // Preserve the bounded data for diagnostics, but make the
            // generation unusable for a gate until the host reconciles it.
            complete = false;
        }
        if references_truncated {
            complete = false;
        }

        let artifacts = self.read_artifacts(
            &referenced_artifacts,
            request.limits,
            request.include_artifact_content,
            &mut complete,
        )?;
        let observed_at_ms = observation.snapshot.updated_at_ms;
        let mut snapshot = EvidenceSnapshotV1 {
            schema: EVIDENCE_SNAPSHOT_SCHEMA_V1.to_string(),
            target: request.target,
            observed_at_ms,
            after_sequence: request.after_sequence,
            first_available_sequence: observation_first_available,
            latest_sequence_exclusive: observation_latest_sequence_exclusive,
            state,
            facts,
            events,
            artifacts,
            complete,
            retention_gap: observation.page.retention_gap,
            snapshot_digest: String::new(),
        };
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn read_artifacts(
        &self,
        references: &BTreeSet<String>,
        limits: EvidenceLimitsV1,
        include_content: bool,
        complete: &mut bool,
    ) -> Result<Vec<EvidenceArtifactV1>, EvidenceError> {
        let Some(store) = &self.artifacts else {
            if !references.is_empty() {
                *complete = false;
            }
            return Ok(Vec::new());
        };
        let mut artifacts = Vec::new();
        let mut total_bytes = 0usize;
        // ArtifactStore preserves insertion order, while the evidence wire
        // contract is canonical URI order. Sort before applying byte/count
        // limits so the selected projection is deterministic as well as
        // validatable.
        let mut stored_artifacts = store.artifacts();
        stored_artifacts.sort_by(|left, right| left.artifact_uri.cmp(&right.artifact_uri));
        for artifact in stored_artifacts {
            if !references.contains(&artifact.artifact_uri) {
                continue;
            }
            if artifacts.len() >= limits.max_artifacts {
                *complete = false;
                break;
            }
            let content_bytes = artifact.content.len();
            let digest = digest_bytes(
                "a3s.code.evidence.artifact-content.v1",
                artifact.content.as_bytes(),
            );
            let content = if include_content
                && content_bytes <= limits.max_artifact_bytes
                && total_bytes.saturating_add(content_bytes) <= limits.max_artifact_bytes
            {
                total_bytes = total_bytes.saturating_add(content_bytes);
                Some(artifact.content.clone())
            } else {
                if include_content {
                    *complete = false;
                }
                None
            };
            artifacts.push(EvidenceArtifactV1 {
                artifact_uri: artifact.artifact_uri,
                tool_name: artifact.tool_name,
                content_digest: digest,
                content_bytes: u64::try_from(content_bytes)
                    .map_err(|_| EvidenceError::NumericOverflow)?,
                content,
            });
        }
        if artifacts.len() < references.len() {
            *complete = false;
        }
        Ok(artifacts)
    }
}

#[async_trait]
impl EvidenceReader for RunEvidenceReader {
    async fn read(
        &self,
        request: EvidenceReadRequestV1,
    ) -> Result<EvidenceSnapshotV1, EvidenceError> {
        self.read(request).await
    }
}

fn bounded_string(value: &str, limit: usize) -> Option<String> {
    (value.len() <= limit).then(|| value.to_string())
}

fn state_content_truncated(
    snapshot: &RunSnapshot,
    limits: EvidenceLimitsV1,
    include_prompt: bool,
    include_terminal_text: bool,
) -> bool {
    (include_prompt && snapshot.prompt.len() > limits.max_prompt_bytes)
        || (include_terminal_text
            && (snapshot
                .result_text
                .as_ref()
                .is_some_and(|value| value.len() > limits.max_result_bytes)
                || snapshot
                    .error
                    .as_ref()
                    .is_some_and(|value| value.len() > limits.max_result_bytes)))
}

fn is_redacted_payload(value: &Value, digest: &str, bytes: u64) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 3
        && object.get("content") == Some(&Value::String("redacted".to_string()))
        && object.get("digest").and_then(Value::as_str) == Some(digest)
        && object.get("bytes").and_then(Value::as_u64) == Some(bytes)
}

fn validate_event_metadata(
    event: &EventEnvelopeV1,
    target: &ExecutionTargetV1,
    sequence: u64,
    occurred_at_ms: u64,
) -> Result<(), EvidenceError> {
    let metadata = event
        .metadata
        .as_ref()
        .ok_or(EvidenceError::InvalidField("event.metadata"))?;
    let encoded = serde_json::to_vec(metadata).map_err(EvidenceError::Serialization)?;
    if encoded.len() > MAX_EVENT_METADATA_BYTES {
        return Err(EvidenceError::InvalidLimit);
    }
    let object = metadata
        .as_object()
        .ok_or(EvidenceError::InvalidField("event.metadata"))?;
    let exact = object.get("run_id").and_then(Value::as_str) == Some(target.run_id.as_str())
        && object.get("session_id").and_then(Value::as_str) == Some(target.session_id.as_str())
        && object.get("sequence").and_then(Value::as_u64) == Some(sequence)
        && object.get("timestamp_ms").and_then(Value::as_u64) == Some(occurred_at_ms);
    if !exact {
        return Err(EvidenceError::TargetMismatch);
    }
    Ok(())
}

/// Compare an unredacted event projection with the digest-only fact that was
/// captured from the same runtime event. The fact and evidence layers use
/// different wire domains, so rebuild the original AgentEvent JSON shape by
/// restoring its top-level `type` field. Redacted payload markers deliberately
/// skip this check: their digest is the retained source commitment, not the
/// marker's digest.
fn fact_payload_matches_event(fact: &ExecutionFactV1, event: &EvidenceEventV1) -> bool {
    if is_redacted_payload(
        &event.event.payload,
        &event.payload_digest,
        event.payload_bytes,
    ) {
        return true;
    }
    let Some(mut object) = event.event.payload.as_object().cloned() else {
        return false;
    };
    object.insert(
        "type".to_string(),
        Value::String(event.event.event_type.clone()),
    );
    // Deserialize back through the runtime enum before serializing. This
    // restores the exact variant field order used when the fact digest was
    // captured; serializing the intermediate JSON map directly can reorder
    // keys and produce a different byte digest for the same event.
    let Ok(runtime_event) = serde_json::from_value::<AgentEvent>(Value::Object(object)) else {
        return false;
    };
    let Ok(encoded) = serde_json::to_vec(&runtime_event) else {
        return false;
    };
    u64::try_from(encoded.len()).ok() == Some(fact.payload_bytes)
        && digest_bytes("a3s.code.execution-fact.payload.v1", &encoded) == fact.payload_digest
}

fn collect_refs_from_value(value: &Value, refs: &mut BTreeSet<String>) -> bool {
    let mut truncated = false;
    collect_refs_from_value_inner(value, refs, &mut truncated);
    truncated
}

fn collect_refs_from_value_inner(value: &Value, refs: &mut BTreeSet<String>, truncated: &mut bool) {
    match value {
        Value::Object(object) => {
            for key in ["artifact_uri", "content_ref", "content_uri"] {
                if let Some(uri) = object.get(key).and_then(Value::as_str) {
                    if uri.is_empty() || uri.len() > MAX_ARTIFACT_URI_BYTES {
                        *truncated = true;
                    } else if refs.len() < MAX_REFERENCED_ARTIFACTS {
                        refs.insert(uri.to_string());
                    } else if !refs.contains(uri) {
                        *truncated = true;
                    }
                }
            }
            for child in object.values() {
                collect_refs_from_value_inner(child, refs, truncated);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_refs_from_value_inner(child, refs, truncated);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
