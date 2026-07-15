//! Versioned wire protocol for [`AgentEvent`].
//!
//! `AgentEvent` is the runtime enum. [`EventEnvelopeV1`] is its stable,
//! language-neutral representation. SDKs consume the envelope instead of
//! maintaining their own event-name matches, so a new runtime event cannot be
//! silently projected as `unknown`.

use crate::agent::AgentEvent;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Wire version carried by [`EventEnvelopeV1`].
pub const EVENT_ENVELOPE_V1_VERSION: u16 = 1;

macro_rules! define_agent_event_types_v1 {
    ($( $variant:ident => $constant:ident = $wire_name:literal ),+ $(,)?) => {
        /// Canonical event type names for event envelope version 1.
        ///
        /// SDK constants and parity checks are generated from this catalog.
        #[derive(Debug, Clone, Copy)]
        pub struct AgentEventTypeV1;

        impl AgentEventTypeV1 {
            $(
                pub const $constant: &'static str = $wire_name;
            )+
        }

        /// Complete, ordered set of event type names supported by envelope v1.
        pub const AGENT_EVENT_TYPES_V1: &[&str] = &[
            $(AgentEventTypeV1::$constant),+
        ];

        impl AgentEvent {
            /// Return the canonical version-1 wire type for this event.
            ///
            /// This match is deliberately exhaustive. Adding an `AgentEvent`
            /// variant without assigning it a stable wire name is a compile
            /// error rather than an SDK event named `unknown`.
            pub const fn event_type_v1(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => AgentEventTypeV1::$constant),+
                }
            }
        }
    };
}

define_agent_event_types_v1! {
    Start => AGENT_START = "agent_start",
    AgentModeChanged => AGENT_MODE_CHANGED = "agent_mode_changed",
    TurnStart => TURN_START = "turn_start",
    TextDelta => TEXT_DELTA = "text_delta",
    ReasoningDelta => REASONING_DELTA = "reasoning_delta",
    ToolStart => TOOL_START = "tool_start",
    ToolInputDelta => TOOL_INPUT_DELTA = "tool_input_delta",
    ToolExecutionStart => TOOL_EXECUTION_START = "tool_execution_start",
    ToolEnd => TOOL_END = "tool_end",
    ToolOutputDelta => TOOL_OUTPUT_DELTA = "tool_output_delta",
    TurnEnd => TURN_END = "turn_end",
    End => AGENT_END = "agent_end",
    Error => ERROR = "error",
    ConfirmationRequired => CONFIRMATION_REQUIRED = "confirmation_required",
    ConfirmationReceived => CONFIRMATION_RECEIVED = "confirmation_received",
    ConfirmationTimeout => CONFIRMATION_TIMEOUT = "confirmation_timeout",
    ExternalTaskPending => EXTERNAL_TASK_PENDING = "external_task_pending",
    ExternalTaskCompleted => EXTERNAL_TASK_COMPLETED = "external_task_completed",
    PermissionDenied => PERMISSION_DENIED = "permission_denied",
    ContextResolving => CONTEXT_RESOLVING = "context_resolving",
    ContextResolved => CONTEXT_RESOLVED = "context_resolved",
    CommandDeadLettered => COMMAND_DEAD_LETTERED = "command_dead_lettered",
    CommandRetry => COMMAND_RETRY = "command_retry",
    QueueAlert => QUEUE_ALERT = "queue_alert",
    TaskUpdated => TASK_UPDATED = "task_updated",
    MemoryStored => MEMORY_STORED = "memory_stored",
    MemoryRecalled => MEMORY_RECALLED = "memory_recalled",
    MemoriesSearched => MEMORIES_SEARCHED = "memories_searched",
    MemoryCleared => MEMORY_CLEARED = "memory_cleared",
    SubagentStart => SUBAGENT_START = "subagent_start",
    SubagentProgress => SUBAGENT_PROGRESS = "subagent_progress",
    SubagentEnd => SUBAGENT_END = "subagent_end",
    PlanningStart => PLANNING_START = "planning_start",
    PlanningEnd => PLANNING_END = "planning_end",
    StepStart => STEP_START = "step_start",
    StepEnd => STEP_END = "step_end",
    GoalExtracted => GOAL_EXTRACTED = "goal_extracted",
    GoalProgress => GOAL_PROGRESS = "goal_progress",
    GoalAchieved => GOAL_ACHIEVED = "goal_achieved",
    ContextCompacted => CONTEXT_COMPACTED = "context_compacted",
    PersistenceFailed => PERSISTENCE_FAILED = "persistence_failed",
    BudgetThresholdHit => BUDGET_THRESHOLD_HIT = "budget_threshold_hit",
    PassivationRequested => PASSIVATION_REQUESTED = "passivation_requested",
    PeerInvocation => PEER_INVOCATION = "peer_invocation",
}

/// Errors produced while converting a runtime event to the stable wire shape.
#[derive(Debug, Error)]
pub enum EventProtocolError {
    #[error("failed to serialize agent event: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("serialized AgentEvent must be a JSON object with a string `type` field")]
    InvalidRuntimeShape,

    #[error(
        "AgentEvent wire type drifted: canonical type is `{canonical}`, serde emitted `{serialized}`"
    )]
    TypeMismatch {
        canonical: &'static str,
        serialized: String,
    },
}

/// Stable, versioned event representation shared by every SDK.
///
/// `event_type` is intentionally an open string. Deserializers therefore keep
/// future event types and their complete payload instead of collapsing them to
/// an `unknown` sentinel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventEnvelopeV1 {
    pub version: u16,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl<'de> Deserialize<'de> for EventEnvelopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireEnvelope {
            version: u16,
            #[serde(rename = "type")]
            event_type: String,
            payload: Value,
            #[serde(default)]
            metadata: Option<Value>,
        }

        let wire = WireEnvelope::deserialize(deserializer)?;
        if wire.version != EVENT_ENVELOPE_V1_VERSION {
            return Err(D::Error::custom(format_args!(
                "unsupported event envelope version {}; expected {}",
                wire.version, EVENT_ENVELOPE_V1_VERSION
            )));
        }

        Ok(Self {
            version: wire.version,
            event_type: wire.event_type,
            payload: wire.payload,
            metadata: wire.metadata,
        })
    }
}

impl EventEnvelopeV1 {
    /// Construct an envelope for a known or future event type.
    pub fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            version: EVENT_ENVELOPE_V1_VERSION,
            event_type: event_type.into(),
            payload,
            metadata: None,
        }
    }

    /// Attach optional protocol metadata such as run or correlation context.
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

impl TryFrom<&AgentEvent> for EventEnvelopeV1 {
    type Error = EventProtocolError;

    fn try_from(event: &AgentEvent) -> Result<Self, Self::Error> {
        let canonical = event.event_type_v1();
        let Value::Object(mut serialized) = serde_json::to_value(event)? else {
            return Err(EventProtocolError::InvalidRuntimeShape);
        };
        let Some(Value::String(serialized_type)) = serialized.remove("type") else {
            return Err(EventProtocolError::InvalidRuntimeShape);
        };
        if serialized_type != canonical {
            return Err(EventProtocolError::TypeMismatch {
                canonical,
                serialized: serialized_type,
            });
        }

        Ok(Self::new(canonical, Value::Object(serialized)))
    }
}

impl TryFrom<AgentEvent> for EventEnvelopeV1 {
    type Error = EventProtocolError;

    fn try_from(event: AgentEvent) -> Result<Self, Self::Error> {
        Self::try_from(&event)
    }
}

/// Convert a persisted run event into the same v1 envelope used by live SDK
/// streams, attaching replay position and correlation metadata.
pub fn run_event_envelope_v1(
    record: &crate::run::RunEventRecord,
    run_id: &str,
    session_id: &str,
) -> Result<EventEnvelopeV1, EventProtocolError> {
    Ok(
        EventEnvelopeV1::try_from(&record.event)?.with_metadata(serde_json::json!({
            "run_id": run_id,
            "session_id": session_id,
            "sequence": record.sequence,
            "timestamp_ms": record.timestamp_ms,
        })),
    )
}

/// SDK-facing projection of an envelope.
///
/// The canonical fields (`version`, `event_type`, `payload`, `metadata`) are
/// lossless. Remaining fields retain the pre-v1 SDK conveniences and are
/// derived centrally so Node and Python cannot disagree about them.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEventProjectionV1 {
    pub version: u16,
    pub event_type: String,
    pub payload: Value,
    pub metadata: Option<Value>,
    pub payload_json: String,
    pub metadata_json: Option<String>,
    /// Legacy SDK `data` view. Events whose payload is not completely
    /// represented by convenience fields keep the full payload here.
    /// Unknown future event types always retain their payload.
    pub data_json: Option<String>,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub tool_output: Option<String>,
    pub exit_code: Option<i32>,
    pub turn: Option<usize>,
    pub prompt: Option<String>,
    pub error: Option<String>,
    pub total_tokens: Option<usize>,
    pub verification_summary_json: Option<String>,
    pub verification_summary_text: Option<String>,
    pub error_kind_json: Option<String>,
}

impl AgentEventProjectionV1 {
    fn string(payload: &Value, key: &str) -> Option<String> {
        payload.get(key)?.as_str().map(ToOwned::to_owned)
    }

    fn usize(payload: &Value, key: &str) -> Option<usize> {
        usize::try_from(payload.get(key)?.as_u64()?).ok()
    }

    fn i32(payload: &Value, key: &str) -> Option<i32> {
        i32::try_from(payload.get(key)?.as_i64()?).ok()
    }
}

impl From<EventEnvelopeV1> for AgentEventProjectionV1 {
    fn from(envelope: EventEnvelopeV1) -> Self {
        let payload_json = envelope.payload.to_string();
        let metadata_json = envelope.metadata.as_ref().map(Value::to_string);
        let data_json = match envelope.event_type.as_str() {
            AgentEventTypeV1::AGENT_START
            | AgentEventTypeV1::TURN_START
            | AgentEventTypeV1::TEXT_DELTA
            | AgentEventTypeV1::REASONING_DELTA
            | AgentEventTypeV1::TOOL_START
            | AgentEventTypeV1::TOOL_INPUT_DELTA
            | AgentEventTypeV1::TOOL_OUTPUT_DELTA
            | AgentEventTypeV1::TURN_END
            | AgentEventTypeV1::AGENT_END
            | AgentEventTypeV1::ERROR
            | AgentEventTypeV1::PLANNING_START => None,
            _ => Some(payload_json.clone()),
        };
        let mut projection = Self {
            version: envelope.version,
            event_type: envelope.event_type,
            payload: envelope.payload,
            metadata: envelope.metadata,
            payload_json,
            metadata_json,
            data_json,
            text: None,
            tool_name: None,
            tool_id: None,
            tool_output: None,
            exit_code: None,
            turn: None,
            prompt: None,
            error: None,
            total_tokens: None,
            verification_summary_json: None,
            verification_summary_text: None,
            error_kind_json: None,
        };

        match projection.event_type.as_str() {
            AgentEventTypeV1::AGENT_START | AgentEventTypeV1::PLANNING_START => {
                projection.prompt = Self::string(&projection.payload, "prompt");
            }
            AgentEventTypeV1::TURN_START => {
                projection.turn = Self::usize(&projection.payload, "turn");
            }
            AgentEventTypeV1::TEXT_DELTA | AgentEventTypeV1::REASONING_DELTA => {
                projection.text = Self::string(&projection.payload, "text");
            }
            AgentEventTypeV1::TOOL_START => {
                projection.tool_id = Self::string(&projection.payload, "id");
                projection.tool_name = Self::string(&projection.payload, "name");
            }
            AgentEventTypeV1::TOOL_INPUT_DELTA => {
                projection.tool_id = Self::string(&projection.payload, "id");
                projection.text = Self::string(&projection.payload, "delta");
            }
            AgentEventTypeV1::TOOL_EXECUTION_START => {
                projection.tool_id = Self::string(&projection.payload, "id");
                projection.tool_name = Self::string(&projection.payload, "name");
            }
            AgentEventTypeV1::TOOL_END => {
                projection.tool_id = Self::string(&projection.payload, "id");
                projection.tool_name = Self::string(&projection.payload, "name");
                projection.tool_output = Self::string(&projection.payload, "output");
                projection.exit_code = Self::i32(&projection.payload, "exit_code");
                projection.error_kind_json = projection
                    .payload
                    .get("error_kind")
                    .filter(|value| !value.is_null())
                    .map(Value::to_string);
            }
            AgentEventTypeV1::TOOL_OUTPUT_DELTA => {
                projection.tool_id = Self::string(&projection.payload, "id");
                projection.tool_name = Self::string(&projection.payload, "name");
                projection.text = Self::string(&projection.payload, "delta");
            }
            AgentEventTypeV1::TURN_END => {
                projection.turn = Self::usize(&projection.payload, "turn");
                projection.total_tokens = projection
                    .payload
                    .get("usage")
                    .and_then(|usage| Self::usize(usage, "total_tokens"));
            }
            AgentEventTypeV1::AGENT_END => {
                projection.text = Self::string(&projection.payload, "text");
                projection.total_tokens = projection
                    .payload
                    .get("usage")
                    .and_then(|usage| Self::usize(usage, "total_tokens"));
                if let Some(summary) = projection.payload.get("verification_summary") {
                    projection.verification_summary_json = Some(summary.to_string());
                    projection.verification_summary_text = serde_json::from_value(summary.clone())
                        .ok()
                        .map(|summary| crate::verification::format_verification_summary(&summary));
                }
            }
            AgentEventTypeV1::ERROR => {
                projection.error = Self::string(&projection.payload, "message");
            }
            AgentEventTypeV1::CONFIRMATION_REQUIRED | AgentEventTypeV1::PERMISSION_DENIED => {
                projection.tool_id = Self::string(&projection.payload, "tool_id");
                projection.tool_name = Self::string(&projection.payload, "tool_name");
            }
            AgentEventTypeV1::CONFIRMATION_RECEIVED | AgentEventTypeV1::CONFIRMATION_TIMEOUT => {
                projection.tool_id = Self::string(&projection.payload, "tool_id");
            }
            AgentEventTypeV1::SUBAGENT_START => {
                projection.tool_id = Self::string(&projection.payload, "task_id");
                projection.tool_name = Self::string(&projection.payload, "agent");
                projection.text = Self::string(&projection.payload, "session_id");
                projection.prompt = Self::string(&projection.payload, "description");
            }
            AgentEventTypeV1::SUBAGENT_PROGRESS => {
                projection.tool_id = Self::string(&projection.payload, "task_id");
                if let (Some(session_id), Some(status)) = (
                    Self::string(&projection.payload, "session_id"),
                    Self::string(&projection.payload, "status"),
                ) {
                    projection.text = Some(format!("{session_id}: {status}"));
                }
            }
            AgentEventTypeV1::SUBAGENT_END => {
                projection.tool_id = Self::string(&projection.payload, "task_id");
                projection.tool_name = Self::string(&projection.payload, "agent");
                projection.text = Self::string(&projection.payload, "session_id");
                projection.tool_output = Self::string(&projection.payload, "output");
                projection.exit_code = projection
                    .payload
                    .get("success")
                    .and_then(Value::as_bool)
                    .map(|success| if success { 0 } else { 1 });
            }
            _ => {}
        }

        projection
    }
}

impl TryFrom<&AgentEvent> for AgentEventProjectionV1 {
    type Error = EventProtocolError;

    fn try_from(event: &AgentEvent) -> Result<Self, Self::Error> {
        EventEnvelopeV1::try_from(event).map(Self::from)
    }
}

impl TryFrom<AgentEvent> for AgentEventProjectionV1 {
    type Error = EventProtocolError;

    fn try_from(event: AgentEvent) -> Result<Self, Self::Error> {
        Self::try_from(&event)
    }
}
