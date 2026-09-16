//! Versioned headless protocol owned by A3S Code.
//!
//! Cloud and other hosts may transport these values, but A3S Code remains the
//! authority for Agent session/run lifecycle, event names, cancellation, and
//! checkpoint recovery. The protocol intentionally contains no Cloud tenant,
//! scheduler, Workload, Runtime, or provider identity.

use crate::event_protocol::{run_event_envelope_v1, EventEnvelopeV1, EVENT_ENVELOPE_V1_VERSION};
pub use crate::release::AGENT_PROTOCOL_V1;
use crate::run::{RunEventPage, RunEventRecord, RunStatus};
use crate::session_checkpoint::{SessionCheckpointDescriptorV1, SessionLogicalResumeEvidenceV1};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const AGENT_PROTOCOL_MAX_ID_BYTES: usize = 256;
pub const AGENT_PROTOCOL_MAX_REASON_BYTES: usize = 1_024;
pub const AGENT_PROTOCOL_MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES: usize = 128;
pub const AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES: usize = 16 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES: usize = 64 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE: usize = 64;
pub const AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES: usize = 6 * 1024 * 1024;
pub const AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES: usize = 4 * 1024 * 1024;
pub const AGENT_PROTOCOL_MAX_CHANGE_SET_RESPONSE_BYTES: usize =
    (AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES * 4 / 3) + 128 * 1024;
pub const AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1: &str = "git_unified_diff_v1";
pub const AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1: &str = "base64";

/// Canonical HTTP endpoint served by `a3s code harness` for v1 commands.
pub const AGENT_PROTOCOL_COMMAND_HTTP_PATH_V1: &str = "/v1/agent/commands";

/// Canonical HTTP endpoint served by `a3s code harness` for v1 event pages.
pub const AGENT_PROTOCOL_EVENT_PAGE_HTTP_PATH_V1: &str = "/v1/agent/events:page";

/// Canonical HTTP endpoint served by `a3s code harness` for immutable run changes.
pub const AGENT_PROTOCOL_CHANGE_SET_HTTP_PATH_V1: &str = "/v1/agent/changes";

/// Stable validation failures for the headless Agent protocol.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentProtocolError {
    #[error("unsupported A3S Code Agent protocol schema")]
    UnsupportedSchema,
    #[error("invalid A3S Code Agent protocol field: {0}")]
    InvalidField(&'static str),
    #[error("A3S Code Agent protocol identity or sequence does not match")]
    IdentityMismatch,
    #[error("A3S Code Agent protocol value exceeds its bounded encoding")]
    Encoding,
}

impl AgentProtocolError {
    /// Stable machine-readable error code for SDK and service boundaries.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "a3s.code.agent_protocol.unsupported_schema",
            Self::InvalidField(_) => "a3s.code.agent_protocol.invalid_field",
            Self::IdentityMismatch => "a3s.code.agent_protocol.identity_mismatch",
            Self::Encoding => "a3s.code.agent_protocol.encoding",
        }
    }
}

/// Exact A3S Code release, session, and run selected by a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolRunIdentityV1 {
    pub schema: String,
    pub protocol: String,
    pub agent_release_identity: String,
    pub session_id: String,
    pub run_id: String,
}

impl AgentProtocolRunIdentityV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-run-identity.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        if self.protocol != AGENT_PROTOCOL_V1 {
            return Err(AgentProtocolError::InvalidField("protocol"));
        }
        validate_lower_sha256("agent_release_identity", &self.agent_release_identity)?;
        validate_id("session_id", &self.session_id)?;
        validate_id("run_id", &self.run_id)
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Start a fresh A3S Code run with an exact host-selected identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolRunStartV1 {
    pub schema: String,
    pub request_id: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub prompt: String,
}

impl AgentProtocolRunStartV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-run-start.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        validate_id("request_id", &self.request_id)?;
        self.identity.validate()?;
        if self.prompt.trim().is_empty()
            || self.prompt.len() > AGENT_PROTOCOL_MAX_PROMPT_BYTES
            || self.prompt.contains('\0')
        {
            return Err(AgentProtocolError::InvalidField("prompt"));
        }
        Ok(())
    }
}

/// Cancel the current exact A3S Code run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolRunCancelV1 {
    pub schema: String,
    pub request_id: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub reason: String,
}

impl AgentProtocolRunCancelV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-run-cancel.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        validate_id("request_id", &self.request_id)?;
        self.identity.validate()?;
        validate_single_line("reason", &self.reason, AGENT_PROTOCOL_MAX_REASON_BYTES)
    }
}

/// Resume an A3S Code loop checkpoint into a fresh exact run identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolRunRecoverV1 {
    pub schema: String,
    pub request_id: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub checkpoint_run_id: String,
}

impl AgentProtocolRunRecoverV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-run-recover.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        validate_id("request_id", &self.request_id)?;
        self.identity.validate()?;
        validate_id("checkpoint_run_id", &self.checkpoint_run_id)?;
        if self.checkpoint_run_id == self.identity.run_id {
            return Err(AgentProtocolError::InvalidField("checkpoint_run_id"));
        }
        Ok(())
    }
}

/// Resume one exact, content-addressed A3S Code tool-round boundary.
///
/// This is an additive recovery request beside [`AgentProtocolRunRecoverV1`].
/// The original request keeps its "latest checkpoint for this run" semantics
/// and wire shape, while this request binds admission and its receipt to the
/// complete secret-free descriptor of one portable Session checkpoint. The
/// descriptor covers the semantic snapshot, logical-resume boundary, and
/// aggregate canonical payload identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolRunRecoverExactV1 {
    pub schema: String,
    pub request_id: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub checkpoint: SessionCheckpointDescriptorV1,
}

impl AgentProtocolRunRecoverExactV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-run-recover-exact.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        validate_id("request_id", &self.request_id)?;
        self.identity.validate()?;
        self.checkpoint
            .validate()
            .map_err(|_| AgentProtocolError::InvalidField("checkpoint"))?;
        let logical_resume = self.logical_resume()?;
        if self.checkpoint.snapshot.session_id != self.identity.session_id
            || logical_resume.session_id != self.identity.session_id
            || logical_resume.source_run_id == self.identity.run_id
        {
            return Err(AgentProtocolError::InvalidField("checkpoint"));
        }
        Ok(())
    }

    pub fn logical_resume(&self) -> Result<&SessionLogicalResumeEvidenceV1, AgentProtocolError> {
        self.checkpoint
            .logical_resume
            .as_ref()
            .ok_or(AgentProtocolError::InvalidField("checkpoint"))
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Closed actions accepted by the version-one Code Agent protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProtocolCommandActionV1 {
    Start,
    Cancel,
    Recover,
}

/// One typed command for the A3S Code session/run lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentProtocolCommandV1 {
    Start { request: AgentProtocolRunStartV1 },
    Cancel { request: AgentProtocolRunCancelV1 },
    Recover { request: AgentProtocolRunRecoverV1 },
}

impl AgentProtocolCommandV1 {
    pub const fn action(&self) -> AgentProtocolCommandActionV1 {
        match self {
            Self::Start { .. } => AgentProtocolCommandActionV1::Start,
            Self::Cancel { .. } => AgentProtocolCommandActionV1::Cancel,
            Self::Recover { .. } => AgentProtocolCommandActionV1::Recover,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Start { request } => &request.request_id,
            Self::Cancel { request } => &request.request_id,
            Self::Recover { request } => &request.request_id,
        }
    }

    pub fn identity(&self) -> &AgentProtocolRunIdentityV1 {
        match self {
            Self::Start { request } => &request.identity,
            Self::Cancel { request } => &request.identity,
            Self::Recover { request } => &request.identity,
        }
    }

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        match self {
            Self::Start { request } => request.validate(),
            Self::Cancel { request } => request.validate(),
            Self::Recover { request } => request.validate(),
        }
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Stable wire projection of [`RunStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProtocolRunStateV1 {
    Created,
    Planning,
    Executing,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl AgentProtocolRunStateV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl From<RunStatus> for AgentProtocolRunStateV1 {
    fn from(value: RunStatus) -> Self {
        match value {
            RunStatus::Created => Self::Created,
            RunStatus::Planning => Self::Planning,
            RunStatus::Executing => Self::Executing,
            RunStatus::Verifying => Self::Verifying,
            RunStatus::Completed => Self::Completed,
            RunStatus::Failed => Self::Failed,
            RunStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Exact observation returned after A3S Code accepts a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolCommandReceiptV1 {
    pub schema: String,
    pub action: AgentProtocolCommandActionV1,
    pub request_id: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub command_digest: String,
    pub state: AgentProtocolRunStateV1,
    pub latest_event_sequence_exclusive: u64,
    pub observed_at_ms: u64,
    pub replayed: bool,
}

impl AgentProtocolCommandReceiptV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-command-receipt.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        validate_id("request_id", &self.request_id)?;
        self.identity.validate()?;
        validate_lower_sha256("command_digest", &self.command_digest)?;
        if self.observed_at_ms == 0 {
            return Err(AgentProtocolError::InvalidField("observed_at_ms"));
        }
        Ok(())
    }

    pub fn validate_for(&self, command: &AgentProtocolCommandV1) -> Result<(), AgentProtocolError> {
        command.validate()?;
        self.validate()?;
        if self.action != command.action()
            || self.request_id != command.request_id()
            || self.identity != *command.identity()
            || self.command_digest != command.digest()?
        {
            return Err(AgentProtocolError::IdentityMismatch);
        }
        if self.action == AgentProtocolCommandActionV1::Cancel && !self.state.is_terminal() {
            return Err(AgentProtocolError::InvalidField("state"));
        }
        Ok(())
    }

    /// Verify that this receipt settles one exact checkpoint recovery request.
    pub fn validate_for_exact_recovery(
        &self,
        request: &AgentProtocolRunRecoverExactV1,
    ) -> Result<(), AgentProtocolError> {
        request.validate()?;
        self.validate()?;
        if self.action != AgentProtocolCommandActionV1::Recover
            || self.request_id != request.request_id
            || self.identity != request.identity
            || self.command_digest != request.digest()?
        {
            return Err(AgentProtocolError::IdentityMismatch);
        }
        Ok(())
    }
}

/// One authoritative A3S Code event at its run-local sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolEventRecordV1 {
    pub sequence: u64,
    pub occurred_at_ms: u64,
    pub event: EventEnvelopeV1,
}

impl AgentProtocolEventRecordV1 {
    pub fn from_run_event(
        record: &RunEventRecord,
        identity: &AgentProtocolRunIdentityV1,
    ) -> Result<Self, AgentProtocolError> {
        identity.validate()?;
        let sequence = u64::try_from(record.sequence)
            .map_err(|_| AgentProtocolError::InvalidField("sequence"))?;
        let event = run_event_envelope_v1(record, &identity.run_id, &identity.session_id)
            .map_err(|_| AgentProtocolError::Encoding)?;
        let mut projected = Self {
            sequence,
            occurred_at_ms: record.timestamp_ms,
            event,
        };
        if projected.validate_for(identity).is_ok() {
            return Ok(projected);
        }
        // Oversized tool_end / text payloads must still project: failing the
        // whole page bricks host observation of terminal run state.
        bound_projected_event_record(&mut projected)?;
        projected.validate_for(identity)?;
        Ok(projected)
    }

    /// Validate this exact record against its Code-owned run identity.
    ///
    /// Hosts use this at durable ingestion boundaries instead of copying the
    /// event metadata and sequence rules into their own protocol layer.
    pub fn validate_for(
        &self,
        identity: &AgentProtocolRunIdentityV1,
    ) -> Result<(), AgentProtocolError> {
        if self.event.version != EVENT_ENVELOPE_V1_VERSION {
            return Err(AgentProtocolError::InvalidField("event.version"));
        }
        validate_single_line(
            "event.type",
            &self.event.event_type,
            AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES,
        )?;
        validate_json_size(
            "event.payload",
            &self.event.payload,
            AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES,
        )?;
        let metadata = self
            .event
            .metadata
            .as_ref()
            .ok_or(AgentProtocolError::InvalidField("event.metadata"))?;
        validate_json_size(
            "event.metadata",
            metadata,
            AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES,
        )?;
        let metadata = metadata
            .as_object()
            .ok_or(AgentProtocolError::InvalidField("event.metadata"))?;
        let exact = metadata.get("session_id").and_then(|value| value.as_str())
            == Some(identity.session_id.as_str())
            && metadata.get("run_id").and_then(|value| value.as_str())
                == Some(identity.run_id.as_str())
            && metadata.get("sequence").and_then(|value| value.as_u64()) == Some(self.sequence)
            && metadata
                .get("timestamp_ms")
                .and_then(|value| value.as_u64())
                == Some(self.occurred_at_ms);
        if !exact {
            return Err(AgentProtocolError::IdentityMismatch);
        }
        let encoded = serde_json::to_vec(self).map_err(|_| AgentProtocolError::Encoding)?;
        if encoded.len() > AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES {
            return Err(AgentProtocolError::InvalidField("event"));
        }
        Ok(())
    }
}

/// Bounded cursor query accepted by the A3S Code Harness event endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolEventPageRequestV1 {
    pub schema: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub after_event_sequence: Option<u64>,
    pub limit: u16,
}

impl AgentProtocolEventPageRequestV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-event-page-request.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        self.identity.validate()?;
        if self.limit == 0 || usize::from(self.limit) > AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE {
            return Err(AgentProtocolError::InvalidField("limit"));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Cursor page projected directly from A3S Code's authoritative run store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolEventPageV1 {
    pub schema: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub after_event_sequence: Option<u64>,
    pub first_available_sequence: Option<u64>,
    pub latest_sequence_exclusive: u64,
    pub next_after_event_sequence: Option<u64>,
    pub state: AgentProtocolRunStateV1,
    pub observed_at_ms: u64,
    pub retention_gap: bool,
    pub has_more: bool,
    pub events: Vec<AgentProtocolEventRecordV1>,
}

impl AgentProtocolEventPageV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-event-page.v1";

    pub fn from_run_page(
        identity: AgentProtocolRunIdentityV1,
        state: RunStatus,
        observed_at_ms: u64,
        after_event_sequence: Option<usize>,
        page: &RunEventPage,
    ) -> Result<Self, AgentProtocolError> {
        identity.validate()?;
        let convert = |value: usize| {
            u64::try_from(value).map_err(|_| AgentProtocolError::InvalidField("sequence"))
        };
        let events = page
            .events
            .iter()
            .map(|record| AgentProtocolEventRecordV1::from_run_event(record, &identity))
            .collect::<Result<Vec<_>, _>>()?;
        let projected = Self {
            schema: Self::SCHEMA.into(),
            identity,
            after_event_sequence: after_event_sequence.map(convert).transpose()?,
            first_available_sequence: page.first_available_sequence.map(convert).transpose()?,
            latest_sequence_exclusive: convert(page.latest_sequence_exclusive)?,
            next_after_event_sequence: page.next_after_sequence.map(convert).transpose()?,
            state: state.into(),
            observed_at_ms,
            retention_gap: page.retention_gap,
            has_more: page.has_more,
            events,
        };
        projected.validate()?;
        Ok(projected)
    }

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        self.identity.validate()?;
        if self.events.len() > AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE {
            return Err(AgentProtocolError::InvalidField("events"));
        }
        if self
            .after_event_sequence
            .is_some_and(|sequence| sequence >= self.latest_sequence_exclusive)
        {
            return Err(AgentProtocolError::InvalidField("after_event_sequence"));
        }
        if self
            .first_available_sequence
            .is_some_and(|sequence| sequence >= self.latest_sequence_exclusive)
        {
            return Err(AgentProtocolError::InvalidField("first_available_sequence"));
        }

        let requested_start = self
            .after_event_sequence
            .map(|sequence| sequence.saturating_add(1))
            .unwrap_or(0);
        let expected_gap = requested_start < self.latest_sequence_exclusive
            && self
                .first_available_sequence
                .is_none_or(|first| requested_start < first);
        if self.retention_gap != expected_gap {
            return Err(AgentProtocolError::InvalidField("retention_gap"));
        }

        let mut previous: Option<(u64, u64)> = None;
        for event in &self.events {
            event.validate_for(&self.identity)?;
            if event.occurred_at_ms > self.observed_at_ms
                || previous.is_some_and(|(sequence, timestamp)| {
                    event.sequence != sequence.saturating_add(1) || event.occurred_at_ms < timestamp
                })
                || event.sequence >= self.latest_sequence_exclusive
            {
                return Err(AgentProtocolError::InvalidField("events"));
            }
            previous = Some((event.sequence, event.occurred_at_ms));
        }

        if let Some(first) = self.events.first() {
            if (!self.retention_gap && first.sequence != requested_start)
                || (self.retention_gap && self.first_available_sequence != Some(first.sequence))
                || self
                    .first_available_sequence
                    .is_some_and(|available| first.sequence < available)
            {
                return Err(AgentProtocolError::InvalidField("events"));
            }
        } else if self.retention_gap && self.first_available_sequence.is_some() {
            return Err(AgentProtocolError::InvalidField("events"));
        }
        let expected_next = self
            .events
            .last()
            .map(|event| event.sequence)
            .or(self.after_event_sequence);
        if self.next_after_event_sequence != expected_next {
            return Err(AgentProtocolError::InvalidField(
                "next_after_event_sequence",
            ));
        }
        if self.has_more {
            if self.events.last().is_none_or(|event| {
                event.sequence.saturating_add(1) >= self.latest_sequence_exclusive
            }) {
                return Err(AgentProtocolError::InvalidField("has_more"));
            }
        } else if let Some(last) = self.events.last() {
            if last.sequence.saturating_add(1) != self.latest_sequence_exclusive {
                return Err(AgentProtocolError::InvalidField("has_more"));
            }
        }
        let encoded = serde_json::to_vec(self).map_err(|_| AgentProtocolError::Encoding)?;
        if encoded.len() > AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES {
            return Err(AgentProtocolError::InvalidField("events"));
        }
        Ok(())
    }

    pub fn first_sequence(&self) -> Option<u64> {
        self.events.first().map(|event| event.sequence)
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.events.last().map(|event| event.sequence)
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Exact run query accepted by the immutable change-set endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolChangeSetRequestV1 {
    pub schema: String,
    pub identity: AgentProtocolRunIdentityV1,
}

impl AgentProtocolChangeSetRequestV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-change-set-request.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        self.identity.validate()
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

/// Immutable Git-compatible unified diff captured for one terminal Code run.
///
/// The base and result tree identities bind the diff to the exact workspace
/// generations observed immediately before and after the run. The content
/// digest and byte count let transports and local apply clients fail closed on
/// truncation or mutation without inventing another run or checkpoint model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProtocolChangeSetV1 {
    pub schema: String,
    pub identity: AgentProtocolRunIdentityV1,
    pub state: AgentProtocolRunStateV1,
    pub format: String,
    pub encoding: String,
    pub base_tree: String,
    pub result_tree: String,
    pub patch_digest: String,
    pub patch_bytes: u64,
    pub patch_base64: String,
    pub observed_at_ms: u64,
}

impl AgentProtocolChangeSetV1 {
    pub const SCHEMA: &'static str = "a3s.code.agent-change-set.v1";

    pub fn validate(&self) -> Result<(), AgentProtocolError> {
        validate_schema(&self.schema, Self::SCHEMA)?;
        self.identity.validate()?;
        if !self.state.is_terminal() {
            return Err(AgentProtocolError::InvalidField("state"));
        }
        if self.format != AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1 {
            return Err(AgentProtocolError::InvalidField("format"));
        }
        if self.encoding != AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1 {
            return Err(AgentProtocolError::InvalidField("encoding"));
        }
        validate_git_tree("base_tree", &self.base_tree)?;
        validate_git_tree("result_tree", &self.result_tree)?;
        validate_lower_sha256("patch_digest", &self.patch_digest)?;
        let declared_bytes = usize::try_from(self.patch_bytes)
            .map_err(|_| AgentProtocolError::InvalidField("patch_bytes"))?;
        let patch = base64::engine::general_purpose::STANDARD
            .decode(&self.patch_base64)
            .map_err(|_| AgentProtocolError::InvalidField("patch_base64"))?;
        if declared_bytes != patch.len()
            || declared_bytes > AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES
            || self.patch_digest != format!("sha256:{:x}", Sha256::digest(&patch))
            || self.observed_at_ms == 0
        {
            return Err(AgentProtocolError::InvalidField("patch_base64"));
        }
        let encoded = serde_json::to_vec(self).map_err(|_| AgentProtocolError::Encoding)?;
        if encoded.len() > AGENT_PROTOCOL_MAX_CHANGE_SET_RESPONSE_BYTES {
            return Err(AgentProtocolError::InvalidField("patch_base64"));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, AgentProtocolError> {
        digest_validated(self, || self.validate())
    }
}

fn validate_schema(value: &str, expected: &str) -> Result<(), AgentProtocolError> {
    if value == expected {
        Ok(())
    } else {
        Err(AgentProtocolError::UnsupportedSchema)
    }
}

fn validate_id(field: &'static str, value: &str) -> Result<(), AgentProtocolError> {
    validate_single_line(field, value, AGENT_PROTOCOL_MAX_ID_BYTES)
}

fn validate_single_line(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), AgentProtocolError> {
    if value.trim().is_empty()
        || value.len() > max
        || value.contains('\0')
        || value.contains(['\r', '\n'])
    {
        Err(AgentProtocolError::InvalidField(field))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_lower_sha256(
    field: &'static str,
    value: &str,
) -> Result<(), AgentProtocolError> {
    let valid = value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(())
    } else {
        Err(AgentProtocolError::InvalidField(field))
    }
}

fn validate_git_tree(field: &'static str, value: &str) -> Result<(), AgentProtocolError> {
    let valid = value.strip_prefix("git-tree:").is_some_and(|hex| {
        matches!(hex.len(), 40 | 64)
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(())
    } else {
        Err(AgentProtocolError::InvalidField(field))
    }
}

fn validate_json_size(
    field: &'static str,
    value: &serde_json::Value,
    max: usize,
) -> Result<(), AgentProtocolError> {
    let encoded = serde_json::to_vec(value).map_err(|_| AgentProtocolError::Encoding)?;
    if encoded.len() > max {
        Err(AgentProtocolError::InvalidField(field))
    } else {
        Ok(())
    }
}

const AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK: &str = "\n…[a3s.code.agent-protocol.truncated]";

/// Shrink oversized payload / non-identity metadata strings until the record
/// validates. Prefer truncating the largest string leaves; if that cannot fit,
/// replace the payload with a bounded stub that preserves small identity fields
/// and a digest of the original payload.
fn bound_projected_event_record(
    projected: &mut AgentProtocolEventRecordV1,
) -> Result<(), AgentProtocolError> {
    let original_payload = projected.event.payload.clone();
    let original_digest = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&original_payload).map_err(|_| AgentProtocolError::Encoding)?
        )
    );

    for _ in 0..128 {
        if projected_record_fits_limits(projected) {
            return Ok(());
        }
        let payload_too_big = validate_json_size(
            "event.payload",
            &projected.event.payload,
            AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES,
        )
        .is_err();
        if payload_too_big {
            if !shrink_largest_json_string(
                &mut projected.event.payload,
                AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK,
            ) {
                projected.event.payload =
                    bounded_event_payload_stub(&original_payload, &original_digest)?;
            }
            continue;
        }
        if let Some(metadata) = projected.event.metadata.as_mut() {
            if !shrink_largest_json_string_excluding(
                metadata,
                AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK,
                &["session_id", "run_id", "sequence", "timestamp_ms"],
            ) {
                // Metadata should stay small; strip non-identity keys.
                if let Some(obj) = metadata.as_object_mut() {
                    obj.retain(|key, _| {
                        matches!(
                            key.as_str(),
                            "session_id" | "run_id" | "sequence" | "timestamp_ms"
                        )
                    });
                }
            }
            continue;
        }
        projected.event.payload = bounded_event_payload_stub(&original_payload, &original_digest)?;
    }

    if projected_record_fits_limits(projected) {
        Ok(())
    } else {
        projected.event.payload = bounded_event_payload_stub(&original_payload, &original_digest)?;
        if projected_record_fits_limits(projected) {
            Ok(())
        } else {
            Err(AgentProtocolError::InvalidField("event"))
        }
    }
}

fn projected_record_fits_limits(projected: &AgentProtocolEventRecordV1) -> bool {
    validate_json_size(
        "event.payload",
        &projected.event.payload,
        AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES,
    )
    .is_ok()
        && projected
            .event
            .metadata
            .as_ref()
            .map(|metadata| {
                validate_json_size(
                    "event.metadata",
                    metadata,
                    AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES,
                )
                .is_ok()
            })
            .unwrap_or(true)
        && serde_json::to_vec(projected)
            .map(|encoded| encoded.len() <= AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES)
            .unwrap_or(false)
}

fn bounded_event_payload_stub(
    original: &serde_json::Value,
    original_digest: &str,
) -> Result<serde_json::Value, AgentProtocolError> {
    let mut stub = serde_json::json!({
        "bounded": true,
        "reason": "agent_protocol_event_payload_limit",
        "original_payload_sha256": original_digest,
    });
    if let Some(obj) = original.as_object() {
        for key in ["id", "name", "exit_code", "tool_id", "tool_name", "turn"] {
            if let Some(value) = obj.get(key) {
                let encoded =
                    serde_json::to_vec(value).map_err(|_| AgentProtocolError::Encoding)?;
                if encoded.len() <= 512 {
                    stub[key] = value.clone();
                }
            }
        }
    }
    Ok(stub)
}

fn shrink_largest_json_string(value: &mut serde_json::Value, mark: &str) -> bool {
    shrink_largest_json_string_excluding(value, mark, &[])
}

fn shrink_largest_json_string_excluding(
    value: &mut serde_json::Value,
    mark: &str,
    excluded_object_keys: &[&str],
) -> bool {
    let mut largest = 0usize;
    largest_string_len(value, excluded_object_keys, &mut largest);
    if largest == 0 {
        return false;
    }
    shrink_first_string_of_len(value, largest, mark, excluded_object_keys)
}

fn largest_string_len(
    value: &serde_json::Value,
    excluded_object_keys: &[&str],
    largest: &mut usize,
) {
    match value {
        serde_json::Value::String(text) => {
            *largest = (*largest).max(text.len());
        }
        serde_json::Value::Array(items) => {
            for item in items {
                largest_string_len(item, excluded_object_keys, largest);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if excluded_object_keys.iter().any(|excluded| *excluded == key) {
                    continue;
                }
                largest_string_len(item, excluded_object_keys, largest);
            }
        }
        _ => {}
    }
}

fn shrink_first_string_of_len(
    value: &mut serde_json::Value,
    target_len: usize,
    mark: &str,
    excluded_object_keys: &[&str],
) -> bool {
    match value {
        serde_json::Value::String(text) if text.len() == target_len => {
            // Halve the largest leaf each pass so oversized tool_end output/args
            // converge under the protocol payload bound without many tiny cuts.
            let keep = (target_len / 2).min(target_len.saturating_sub(mark.len()));
            let boundary = crate::text::truncate_utf8(text, keep).len();
            text.truncate(boundary);
            text.push_str(mark);
            true
        }
        serde_json::Value::Array(items) => {
            for item in items {
                if shrink_first_string_of_len(item, target_len, mark, excluded_object_keys) {
                    return true;
                }
            }
            false
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if excluded_object_keys.iter().any(|excluded| *excluded == key) {
                    continue;
                }
                if shrink_first_string_of_len(item, target_len, mark, excluded_object_keys) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn digest_validated<T: Serialize>(
    value: &T,
    validate: impl FnOnce() -> Result<(), AgentProtocolError>,
) -> Result<String, AgentProtocolError> {
    validate()?;
    let encoded = serde_json::to_vec(value).map_err(|_| AgentProtocolError::Encoding)?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use serde_json::json;

    fn identity() -> AgentProtocolRunIdentityV1 {
        AgentProtocolRunIdentityV1 {
            schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
            protocol: AGENT_PROTOCOL_V1.into(),
            agent_release_identity: format!("sha256:{}", "a".repeat(64)),
            session_id: "conversation-bound-meta".into(),
            run_id: "run-bound-meta".into(),
        }
    }

    #[test]
    fn bound_projected_event_record_strips_oversized_non_identity_metadata() {
        let identity = identity();
        let mut projected = AgentProtocolEventRecordV1 {
            sequence: 0,
            occurred_at_ms: 1,
            event: EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64,
                "noise": "n".repeat(AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES),
            })),
        };

        bound_projected_event_record(&mut projected).expect("metadata must shrink");
        projected
            .validate_for(&identity)
            .expect("bounded metadata record must validate");
        let metadata = projected
            .event
            .metadata
            .as_ref()
            .and_then(|value| value.as_object())
            .expect("metadata object");
        assert_eq!(
            metadata.get("session_id").and_then(|value| value.as_str()),
            Some(identity.session_id.as_str())
        );
        assert_eq!(
            metadata.get("run_id").and_then(|value| value.as_str()),
            Some(identity.run_id.as_str())
        );
        let noise = metadata
            .get("noise")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            noise.len() < AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES,
            "non-identity metadata must shrink below the protocol bound"
        );
        assert!(
            noise.ends_with(AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK)
                || !metadata.contains_key("noise"),
            "oversized metadata must truncate or drop non-identity keys"
        );
    }

    #[test]
    fn from_run_page_projects_oversized_tool_end_instead_of_400() {
        let identity = identity();
        let oversized = "x".repeat(AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES + 8_192);
        let page = RunEventPage {
            events: vec![RunEventRecord {
                sequence: 0,
                timestamp_ms: 1,
                event: AgentEvent::ToolEnd {
                    id: "tool-1".into(),
                    name: "read".into(),
                    args: None,
                    exit_code: 0,
                    output: oversized,
                    metadata: None,
                    error_kind: None,
                },
            }],
            first_available_sequence: Some(0),
            latest_sequence_exclusive: 1,
            next_after_sequence: Some(0),
            retention_gap: false,
            has_more: false,
        };

        let projected = AgentProtocolEventPageV1::from_run_page(
            identity.clone(),
            RunStatus::Completed,
            1,
            None,
            &page,
        )
        .expect("oversized tool_end must still project a page");
        projected.validate().expect("projected page must validate");
        assert_eq!(projected.events.len(), 1);
        assert_eq!(projected.events[0].event.event_type, "tool_end");
        let payload = &projected.events[0].event.payload;
        let encoded = serde_json::to_vec(payload).expect("encode payload");
        assert!(
            encoded.len() <= AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES,
            "bounded payload must fit protocol limit (got {})",
            encoded.len()
        );
        assert_eq!(
            payload.get("name").and_then(|value| value.as_str()),
            Some("read"),
            "identity fields must survive bounding"
        );
    }

    #[test]
    fn protocol_error_codes_are_stable() {
        assert_eq!(
            AgentProtocolError::UnsupportedSchema.code(),
            "a3s.code.agent_protocol.unsupported_schema"
        );
        assert_eq!(
            AgentProtocolError::InvalidField("prompt").code(),
            "a3s.code.agent_protocol.invalid_field"
        );
        assert_eq!(
            AgentProtocolError::IdentityMismatch.code(),
            "a3s.code.agent_protocol.identity_mismatch"
        );
        assert_eq!(
            AgentProtocolError::Encoding.code(),
            "a3s.code.agent_protocol.encoding"
        );
    }

    #[test]
    fn identity_rejects_foreign_protocol_and_digests_when_valid() {
        let mut bad = identity();
        bad.protocol = "a3s.code.agent.v0".into();
        assert_eq!(
            bad.validate(),
            Err(AgentProtocolError::InvalidField("protocol"))
        );
        let good = identity();
        let digest = good.digest().expect("valid identity digests");
        assert!(digest.starts_with("sha256:"));
    }

    #[test]
    fn event_page_validation_rejects_inconsistent_cursors_and_flags() {
        let identity = identity();
        let make_event = |sequence: u64, occurred_at_ms: u64| AgentProtocolEventRecordV1 {
            sequence,
            occurred_at_ms,
            event: EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": sequence,
                "timestamp_ms": occurred_at_ms,
            })),
        };
        let mut page = AgentProtocolEventPageV1 {
            schema: AgentProtocolEventPageV1::SCHEMA.into(),
            identity: identity.clone(),
            after_event_sequence: None,
            first_available_sequence: Some(0),
            latest_sequence_exclusive: 1,
            next_after_event_sequence: Some(0),
            state: AgentProtocolRunStateV1::Completed,
            observed_at_ms: 10,
            retention_gap: false,
            has_more: false,
            events: vec![make_event(0, 10)],
        };
        page.validate()
            .expect("baseline page with metadata validates");
        assert_eq!(page.first_sequence(), Some(0));
        assert_eq!(page.last_sequence(), Some(0));
        assert!(page.digest().expect("digest").starts_with("sha256:"));

        // after_event_sequence >= latest is invalid.
        page.after_event_sequence = Some(1);
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("after_event_sequence"))
        );

        page.after_event_sequence = None;
        page.first_available_sequence = Some(1);
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("first_available_sequence"))
        );

        page.first_available_sequence = Some(0);
        page.retention_gap = true;
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("retention_gap"))
        );

        page.retention_gap = false;
        page.next_after_event_sequence = Some(9);
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField(
                "next_after_event_sequence"
            ))
        );

        page.next_after_event_sequence = Some(0);
        page.has_more = true;
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("has_more"))
        );

        page.has_more = false;
        page.latest_sequence_exclusive = 3;
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("has_more"))
        );

        // Non-contiguous event sequence after a valid cursor.
        page.latest_sequence_exclusive = 3;
        page.after_event_sequence = None;
        page.first_available_sequence = Some(0);
        page.next_after_event_sequence = Some(1);
        page.events = vec![make_event(0, 1), make_event(2, 2)];
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("events"))
        );

        // Empty events with retention_gap + first_available is invalid once the
        // retention_gap flag itself is consistent with the cursor math.
        page.events.clear();
        page.after_event_sequence = None;
        page.first_available_sequence = Some(1);
        page.latest_sequence_exclusive = 2;
        page.next_after_event_sequence = None;
        page.retention_gap = true;
        page.has_more = false;
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("events"))
        );
    }

    #[test]
    fn shrink_helpers_walk_arrays_and_objects() {
        let mut value = json!({
            "keep": "identity",
            "items": ["short", "this-string-is-long-enough-to-shrink-aaaaaaaa"],
            "nested": {"noise": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
        });
        assert!(shrink_largest_json_string_excluding(
            &mut value,
            AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK,
            &["keep"],
        ));
        let mutated = value.to_string();
        assert!(
            mutated.contains("a3s.code.agent-protocol.truncated"),
            "expected truncation mark in {mutated}"
        );

        let mut empty = json!({"n": 1, "b": true, "z": null});
        assert!(!shrink_largest_json_string(
            &mut empty,
            AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK
        ));
    }

    #[test]
    fn bounded_event_payload_stub_preserves_small_identity_fields() {
        let original = json!({
            "id": "tool-1",
            "name": "read",
            "exit_code": 0,
            "tool_id": "t",
            "tool_name": "read",
            "turn": 2,
            "huge": "x".repeat(1024),
        });
        let digest = format!("sha256:{}", "ab".repeat(32));
        let stub = bounded_event_payload_stub(&original, &digest).expect("stub");
        assert_eq!(stub["bounded"], true);
        assert_eq!(stub["original_payload_sha256"], digest);
        assert_eq!(stub["id"], "tool-1");
        assert_eq!(stub["name"], "read");
        assert!(stub.get("huge").is_none());
    }

    #[test]
    fn bound_projected_event_record_falls_back_to_stub_for_non_string_payload() {
        let identity = identity();
        let mut projected = AgentProtocolEventRecordV1 {
            sequence: 0,
            occurred_at_ms: 1,
            event: EventEnvelopeV1::new(
                "tool_end",
                json!({
                    "id": "tool-1",
                    "name": "read",
                    "exit_code": 0,
                    "blob": {"n": 1},
                    "pads": (0..80).map(|i| format!("pad-{i}-{}", "z".repeat(512))).collect::<Vec<_>>(),
                }),
            )
            .with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64,
                "extra": (0..40).map(|i| format!("meta-{i}-{}", "m".repeat(256))).collect::<Vec<_>>(),
            })),
        };
        bound_projected_event_record(&mut projected).expect("must bound");
        projected
            .validate_for(&identity)
            .expect("bounded record validates");
    }

    #[test]
    fn recover_rejects_checkpoint_run_id_equal_to_target_run() {
        let identity = identity();
        let recover = AgentProtocolRunRecoverV1 {
            schema: AgentProtocolRunRecoverV1::SCHEMA.into(),
            request_id: "req-recover".into(),
            identity: identity.clone(),
            checkpoint_run_id: identity.run_id.clone(),
        };
        assert_eq!(
            recover.validate(),
            Err(AgentProtocolError::InvalidField("checkpoint_run_id"))
        );
    }

    #[test]
    fn run_state_from_covers_every_run_status_variant() {
        use crate::run::RunStatus;
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Created),
            AgentProtocolRunStateV1::Created
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Planning),
            AgentProtocolRunStateV1::Planning
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Executing),
            AgentProtocolRunStateV1::Executing
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Verifying),
            AgentProtocolRunStateV1::Verifying
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Completed),
            AgentProtocolRunStateV1::Completed
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Failed),
            AgentProtocolRunStateV1::Failed
        );
        assert_eq!(
            AgentProtocolRunStateV1::from(RunStatus::Cancelled),
            AgentProtocolRunStateV1::Cancelled
        );
        assert!(AgentProtocolRunStateV1::Completed.is_terminal());
        assert!(!AgentProtocolRunStateV1::Executing.is_terminal());
    }

    #[test]
    fn receipt_validation_rejects_zero_observed_at_and_nonterminal_cancel() {
        let command = AgentProtocolCommandV1::Cancel {
            request: AgentProtocolRunCancelV1 {
                schema: AgentProtocolRunCancelV1::SCHEMA.into(),
                request_id: "req-cancel".into(),
                identity: identity(),
                reason: "user".into(),
            },
        };
        let mut receipt = AgentProtocolCommandReceiptV1 {
            schema: AgentProtocolCommandReceiptV1::SCHEMA.into(),
            action: AgentProtocolCommandActionV1::Cancel,
            request_id: "req-cancel".into(),
            identity: identity(),
            command_digest: command.digest().expect("digest"),
            state: AgentProtocolRunStateV1::Cancelled,
            latest_event_sequence_exclusive: 1,
            observed_at_ms: 0,
            replayed: false,
        };
        assert_eq!(
            receipt.validate(),
            Err(AgentProtocolError::InvalidField("observed_at_ms"))
        );
        receipt.observed_at_ms = 10;
        receipt.state = AgentProtocolRunStateV1::Executing;
        assert_eq!(
            receipt.validate_for(&command),
            Err(AgentProtocolError::InvalidField("state"))
        );
    }

    #[test]
    fn event_record_validate_for_rejects_bad_version_type_and_metadata() {
        let identity = identity();
        let mut record = AgentProtocolEventRecordV1 {
            sequence: 0,
            occurred_at_ms: 1,
            event: EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64,
            })),
        };
        record.event.version = 99;
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.version"))
        );
        record.event.version = EVENT_ENVELOPE_V1_VERSION;
        record.event.event_type = "bad\ntype".into();
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.type"))
        );
        record.event.event_type = "text_delta".into();
        record.event.metadata = None;
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.metadata"))
        );
        record.event.metadata = Some(json!("not-an-object"));
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.metadata"))
        );
        record.event.metadata = Some(json!({
            "session_id": "other",
            "run_id": identity.run_id,
            "sequence": 0u64,
            "timestamp_ms": 1u64,
        }));
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::IdentityMismatch)
        );
    }

    #[test]
    fn event_page_validate_rejects_too_many_events_and_has_more_inconsistency() {
        let identity = identity();
        let make_event = |sequence: u64| AgentProtocolEventRecordV1 {
            sequence,
            occurred_at_ms: sequence + 1,
            event: EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": sequence,
                "timestamp_ms": sequence + 1,
            })),
        };
        let mut page = AgentProtocolEventPageV1 {
            schema: AgentProtocolEventPageV1::SCHEMA.into(),
            identity: identity.clone(),
            after_event_sequence: None,
            first_available_sequence: Some(0),
            latest_sequence_exclusive: 1,
            next_after_event_sequence: Some(0),
            state: AgentProtocolRunStateV1::Completed,
            observed_at_ms: 100,
            retention_gap: false,
            has_more: false,
            events: vec![make_event(0)],
        };
        page.validate().expect("baseline page validates");

        page.events = (0..=AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE as u64)
            .map(make_event)
            .collect();
        page.latest_sequence_exclusive = page.events.len() as u64;
        page.next_after_event_sequence = Some(page.events.len() as u64 - 1);
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("events"))
        );

        // has_more=true requires room after the last event.
        page.events = vec![make_event(0)];
        page.latest_sequence_exclusive = 1;
        page.next_after_event_sequence = Some(0);
        page.has_more = true;
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField("has_more"))
        );

        page.has_more = false;
        page.latest_sequence_exclusive = 1;
        page.next_after_event_sequence = Some(9);
        assert_eq!(
            page.validate(),
            Err(AgentProtocolError::InvalidField(
                "next_after_event_sequence"
            ))
        );
    }

    #[test]
    fn change_set_validate_rejects_format_encoding_and_tree_errors() {
        let patch = b"diff --git a/x b/x\n";
        let mut change_set = AgentProtocolChangeSetV1 {
            schema: AgentProtocolChangeSetV1::SCHEMA.into(),
            identity: identity(),
            state: AgentProtocolRunStateV1::Completed,
            format: AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1.into(),
            encoding: AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1.into(),
            base_tree: format!("git-tree:{}", "1".repeat(40)),
            result_tree: format!("git-tree:{}", "2".repeat(40)),
            patch_digest: format!("sha256:{:x}", Sha256::digest(patch)),
            patch_bytes: patch.len() as u64,
            patch_base64: base64::engine::general_purpose::STANDARD.encode(patch),
            observed_at_ms: 10,
        };
        change_set.validate().expect("valid change set");

        change_set.format = "other".into();
        assert_eq!(
            change_set.validate(),
            Err(AgentProtocolError::InvalidField("format"))
        );
        change_set.format = AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1.into();
        change_set.encoding = "hex".into();
        assert_eq!(
            change_set.validate(),
            Err(AgentProtocolError::InvalidField("encoding"))
        );
        change_set.encoding = AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1.into();
        change_set.base_tree = "not-a-tree".into();
        assert_eq!(
            change_set.validate(),
            Err(AgentProtocolError::InvalidField("base_tree"))
        );
        change_set.base_tree = format!("git-tree:{}", "1".repeat(40));
        change_set.observed_at_ms = 0;
        assert_eq!(
            change_set.validate(),
            Err(AgentProtocolError::InvalidField("patch_base64"))
        );
    }

    #[test]
    fn validators_reject_bad_schema_ids_sha256_and_git_trees() {
        assert_eq!(
            validate_schema("wrong", AgentProtocolRunIdentityV1::SCHEMA),
            Err(AgentProtocolError::UnsupportedSchema)
        );
        assert_eq!(
            validate_id("session_id", ""),
            Err(AgentProtocolError::InvalidField("session_id"))
        );
        assert_eq!(
            validate_id("session_id", "has\nnewline"),
            Err(AgentProtocolError::InvalidField("session_id"))
        );
        assert_eq!(
            validate_lower_sha256("digest", "sha256:zzzz"),
            Err(AgentProtocolError::InvalidField("digest"))
        );
        assert_eq!(
            validate_git_tree("tree", "git-tree:xyz"),
            Err(AgentProtocolError::InvalidField("tree"))
        );
        assert!(validate_git_tree("tree", &format!("git-tree:{}", "a".repeat(64))).is_ok());
    }

    #[test]
    fn change_set_request_digest_binds_validated_identity() {
        let request = AgentProtocolChangeSetRequestV1 {
            schema: AgentProtocolChangeSetRequestV1::SCHEMA.into(),
            identity: identity(),
        };
        request.validate().expect("valid");
        assert!(request.digest().expect("digest").starts_with("sha256:"));
        let mut bad = request.clone();
        bad.schema = "wrong".into();
        assert_eq!(bad.validate(), Err(AgentProtocolError::UnsupportedSchema));
    }

    #[test]
    fn event_record_rejects_oversized_metadata_and_encoded_record() {
        let identity = identity();
        let mut record = AgentProtocolEventRecordV1 {
            sequence: 0,
            occurred_at_ms: 1,
            event: EventEnvelopeV1::new("text_delta", json!({"text": "ok"})).with_metadata(json!({
                "session_id": identity.session_id,
                "run_id": identity.run_id,
                "sequence": 0u64,
                "timestamp_ms": 1u64,
                "noise": "n".repeat(AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES + 64),
            })),
        };
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.metadata"))
        );

        // Keep metadata small but inflate the event type so the encoded record
        // exceeds AGENT_PROTOCOL_MAX_EVENT_RECORD_BYTES after identity checks.
        record.event.metadata = Some(json!({
            "session_id": identity.session_id,
            "run_id": identity.run_id,
            "sequence": 0u64,
            "timestamp_ms": 1u64,
        }));
        record.event.event_type = "t".repeat(AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES + 1);
        // Oversized event_type is rejected by validate_single_line first.
        assert_eq!(
            record.validate_for(&identity),
            Err(AgentProtocolError::InvalidField("event.type"))
        );
    }

    #[test]
    fn bounded_event_payload_stub_skips_oversized_identity_fields_and_non_objects() {
        let digest = format!("sha256:{}", "cd".repeat(32));
        let original = json!({
            "id": "x".repeat(600),
            "name": "read",
        });
        let stub = bounded_event_payload_stub(&original, &digest).expect("stub");
        assert!(
            stub.get("id").is_none(),
            "fields over 512 bytes are skipped"
        );
        assert_eq!(stub["name"], "read");

        let non_object = bounded_event_payload_stub(&json!("plain"), &digest).expect("non-object");
        assert_eq!(non_object["bounded"], true);
        assert!(non_object.get("id").is_none());
    }

    #[test]
    fn shrink_helpers_return_false_when_no_eligible_string_exists() {
        let mut value = json!({
            "session_id": "keep-me-alone",
            "items": [1, true, null],
            "nested": {"run_id": "also-excluded"}
        });
        assert!(!shrink_largest_json_string_excluding(
            &mut value,
            AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK,
            &["session_id", "run_id"],
        ));
        assert!(!shrink_first_string_of_len(
            &mut value,
            999,
            AGENT_PROTOCOL_PAYLOAD_TRUNCATION_MARK,
            &["session_id", "run_id"],
        ));
    }

    #[test]
    fn command_action_and_request_id_cover_recover_variant() {
        let command = AgentProtocolCommandV1::Recover {
            request: AgentProtocolRunRecoverV1 {
                schema: AgentProtocolRunRecoverV1::SCHEMA.into(),
                request_id: "req-recover-variant".into(),
                identity: identity(),
                checkpoint_run_id: "checkpoint-source".into(),
            },
        };
        assert_eq!(command.action(), AgentProtocolCommandActionV1::Recover);
        assert_eq!(command.request_id(), "req-recover-variant");
        command.validate().expect("recover command validates");
        assert!(command.digest().expect("digest").starts_with("sha256:"));
    }

    #[test]
    fn run_state_as_str_covers_every_variant() {
        for (state, name) in [
            (AgentProtocolRunStateV1::Created, "created"),
            (AgentProtocolRunStateV1::Planning, "planning"),
            (AgentProtocolRunStateV1::Executing, "executing"),
            (AgentProtocolRunStateV1::Verifying, "verifying"),
            (AgentProtocolRunStateV1::Completed, "completed"),
            (AgentProtocolRunStateV1::Failed, "failed"),
            (AgentProtocolRunStateV1::Cancelled, "cancelled"),
        ] {
            assert_eq!(state.as_str(), name);
        }
    }

    #[test]
    fn event_page_request_rejects_zero_limit_and_digests_when_valid() {
        let mut request = AgentProtocolEventPageRequestV1 {
            schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
            identity: identity(),
            after_event_sequence: None,
            limit: 16,
        };
        request.validate().expect("valid");
        assert!(request.digest().expect("digest").starts_with("sha256:"));
        request.limit = 0;
        assert_eq!(
            request.validate(),
            Err(AgentProtocolError::InvalidField("limit"))
        );
        request.limit = (AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE as u16).saturating_add(1);
        assert_eq!(
            request.validate(),
            Err(AgentProtocolError::InvalidField("limit"))
        );
    }
}
