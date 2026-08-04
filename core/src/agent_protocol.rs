//! Versioned headless protocol owned by A3S Code.
//!
//! Cloud and other hosts may transport these values, but A3S Code remains the
//! authority for Agent session/run lifecycle, event names, cancellation, and
//! checkpoint recovery. The protocol intentionally contains no Cloud tenant,
//! scheduler, Workload, Runtime, or provider identity.

use crate::event_protocol::{run_event_envelope_v1, EventEnvelopeV1, EVENT_ENVELOPE_V1_VERSION};
pub use crate::release::AGENT_PROTOCOL_V1;
use crate::run::{RunEventPage, RunEventRecord, RunStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const AGENT_PROTOCOL_MAX_ID_BYTES: usize = 256;
pub const AGENT_PROTOCOL_MAX_REASON_BYTES: usize = 1_024;
pub const AGENT_PROTOCOL_MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENT_TYPE_BYTES: usize = 128;
pub const AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENT_METADATA_BYTES: usize = 16 * 1024;
pub const AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE: usize = 64;
pub const AGENT_PROTOCOL_MAX_EVENT_PAGE_BYTES: usize = 6 * 1024 * 1024;

/// Canonical HTTP endpoint served by `a3s code harness` for v1 commands.
pub const AGENT_PROTOCOL_COMMAND_HTTP_PATH_V1: &str = "/v1/agent/commands";

/// Canonical HTTP endpoint served by `a3s code harness` for v1 event pages.
pub const AGENT_PROTOCOL_EVENT_PAGE_HTTP_PATH_V1: &str = "/v1/agent/events:page";

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

    pub fn validate_for(&self, command: &AgentProtocolCommandV1) -> Result<(), AgentProtocolError> {
        command.validate()?;
        validate_schema(&self.schema, Self::SCHEMA)?;
        self.identity.validate()?;
        validate_lower_sha256("command_digest", &self.command_digest)?;
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
        let projected = Self {
            sequence,
            occurred_at_ms: record.timestamp_ms,
            event,
        };
        projected.validate_for(identity)?;
        Ok(projected)
    }

    fn validate_for(
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

        let mut previous = None;
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

fn digest_validated<T: Serialize>(
    value: &T,
    validate: impl FnOnce() -> Result<(), AgentProtocolError>,
) -> Result<String, AgentProtocolError> {
    validate()?;
    let encoded = serde_json::to_vec(value).map_err(|_| AgentProtocolError::Encoding)?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}
