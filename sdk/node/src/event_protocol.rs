//! Node event protocol and agent result projections.

use super::*;

// ============================================================================
// AgentResult
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct AgentResult {
    pub text: String,
    pub tool_calls_count: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub verification_status: String,
    pub pending_verification_count: u32,
    pub failed_verification_count: u32,
    pub verification_report_count: u32,
    pub verification_summary_json: String,
    pub verification_summary_text: String,
}

impl From<RustAgentResult> for AgentResult {
    fn from(r: RustAgentResult) -> Self {
        let verification_summary = r.verification_summary();
        let verification_summary_json = verification_summary.to_value().to_string();
        let verification_summary_text = rust_format_verification_summary(&verification_summary);
        Self {
            text: r.text,
            tool_calls_count: r.tool_calls_count as u32,
            prompt_tokens: r.usage.prompt_tokens as u32,
            completion_tokens: r.usage.completion_tokens as u32,
            total_tokens: r.usage.total_tokens as u32,
            verification_status: verification_status_label(verification_summary.status),
            pending_verification_count: verification_summary.pending_required_check_count as u32,
            failed_verification_count: verification_summary.failed_check_count as u32,
            verification_report_count: verification_summary.report_count as u32,
            verification_summary_json,
            verification_summary_text,
        }
    }
}

fn verification_status_label(status: RustVerificationStatus) -> String {
    match status {
        RustVerificationStatus::Passed => "passed",
        RustVerificationStatus::Failed => "failed",
        RustVerificationStatus::NeedsReview => "needs_review",
        RustVerificationStatus::Skipped => "skipped",
    }
    .to_string()
}

#[napi]
pub fn format_verification_summary(summary: serde_json::Value) -> napi::Result<String> {
    let summary: RustVerificationSummary = match summary {
        serde_json::Value::String(summary_json) => serde_json::from_str(&summary_json),
        value => serde_json::from_value(value),
    }
    .map_err(|e| napi::Error::from_reason(format!("Invalid verification summary: {e}")))?;
    Ok(rust_format_verification_summary(&summary))
}

// ============================================================================
// AgentEvent
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct AgentEvent {
    /// Stable event-envelope protocol version. Currently always `1`.
    pub version: u32,
    #[napi(js_name = "type")]
    pub event_type: String,
    /// Complete, lossless event payload. Unknown future event types retain it.
    pub payload: serde_json::Value,
    /// Optional protocol metadata, independent from the event payload.
    pub metadata: Option<serde_json::Value>,
    /// JSON-encoded form of `payload` for string-oriented consumers.
    pub payload_json: String,
    /// JSON-encoded form of `metadata`, when present.
    pub metadata_json: Option<String>,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub tool_output: Option<String>,
    pub exit_code: Option<i32>,
    pub turn: Option<u32>,
    pub prompt: Option<String>,
    pub error: Option<String>,
    pub total_tokens: Option<u32>,
    pub verification_summary_json: Option<String>,
    pub verification_summary_text: Option<String>,
    /// Legacy JSON view for events not fully represented by convenience fields.
    /// Prefer `payload` or `payloadJson` in new code.
    pub data: Option<String>,
    /// Structured discriminant for tool failures on `tool_end` events
    /// (JSON-encoded with a `type` field). `None` on success or untyped
    /// failure. Lets streaming consumers branch on the failure kind
    /// without scanning `tool_output`.
    pub error_kind_json: Option<String>,
}

#[napi(object)]
#[derive(Clone)]
pub struct VerificationCommand {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub command: String,
    pub required: Option<bool>,
    pub timeout_ms: Option<u32>,
}

impl From<VerificationCommand> for RustVerificationCommand {
    fn from(command: VerificationCommand) -> Self {
        let mut rust_command = if command.required.unwrap_or(true) {
            RustVerificationCommand::required(
                command.id,
                command.kind,
                command.description,
                command.command,
            )
        } else {
            RustVerificationCommand::optional(
                command.id,
                command.kind,
                command.description,
                command.command,
            )
        };

        if let Some(timeout_ms) = command.timeout_ms {
            rust_command = rust_command.with_timeout_ms(timeout_ms as u64);
        }

        rust_command
    }
}

impl TryFrom<RustAgentEvent> for AgentEvent {
    type Error = RustEventProtocolError;

    fn try_from(event: RustAgentEvent) -> Result<Self, Self::Error> {
        let projection = RustAgentEventProjectionV1::try_from(event)?;
        Ok(Self::from_projection(projection))
    }
}

impl AgentEvent {
    fn from_projection(projection: RustAgentEventProjectionV1) -> Self {
        Self {
            version: u32::from(projection.version),
            event_type: projection.event_type,
            payload: projection.payload,
            metadata: projection.metadata,
            payload_json: projection.payload_json,
            metadata_json: projection.metadata_json,
            text: projection.text,
            tool_name: projection.tool_name,
            tool_id: projection.tool_id,
            tool_output: projection.tool_output,
            exit_code: projection.exit_code,
            turn: projection.turn.and_then(|value| u32::try_from(value).ok()),
            prompt: projection.prompt,
            error: projection.error,
            total_tokens: projection
                .total_tokens
                .and_then(|value| u32::try_from(value).ok()),
            verification_summary_json: projection.verification_summary_json,
            verification_summary_text: projection.verification_summary_text,
            data: projection.data_json,
            error_kind_json: projection.error_kind_json,
        }
    }
}

/// Return the canonical version-1 event type catalog.
///
/// AgentEvent.type remains an open string so consumers preserve future event
/// types. This catalog is useful for discovery and protocol parity checks.
#[napi]
pub fn agent_event_types_v1() -> Vec<String> {
    AGENT_EVENT_TYPES_V1
        .iter()
        .map(|event_type| (*event_type).to_string())
        .collect()
}

/// Return the current stable event-envelope protocol version.
#[napi]
pub fn event_envelope_v1_version() -> u32 {
    u32::from(EVENT_ENVELOPE_V1_VERSION)
}

#[cfg(test)]
mod agent_event_protocol_tests {
    use super::*;
    use a3s_code_core::EventEnvelopeV1;
    use serde_json::json;

    #[test]
    fn sdk_event_preserves_unknown_type_payload_and_metadata() {
        let projection = RustAgentEventProjectionV1::from(
            EventEnvelopeV1::new("future_event", json!({ "opaque": [1, 2, 3] }))
                .with_metadata(json!({ "correlation_id": "future-1" })),
        );

        let event = AgentEvent::from_projection(projection);

        assert_eq!(event.version, 1);
        assert_eq!(event.event_type, "future_event");
        assert_eq!(event.payload, json!({ "opaque": [1, 2, 3] }));
        assert_eq!(
            event.metadata,
            Some(json!({ "correlation_id": "future-1" }))
        );
        assert_eq!(event.data.as_deref(), Some(r#"{"opaque":[1,2,3]}"#));
    }

    #[test]
    fn sdk_catalog_is_the_core_catalog() {
        assert_eq!(agent_event_types_v1(), AGENT_EVENT_TYPES_V1);
        assert_eq!(event_envelope_v1_version(), 1);
    }

    #[tokio::test]
    async fn terminal_event_is_yielded_before_stream_exhaustion() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(RustAgentEvent::End {
            text: "done".into(),
            usage: a3s_code_core::TokenUsage::default(),
            verification_summary: Box::new(RustVerificationSummary::from_reports(&[])),
            meta: None,
        })
        .await
        .expect("terminal event should enter the test stream");
        drop(tx);
        let stream = EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(Some(tokio::spawn(async {})))),
        };

        let terminal = stream.next().await.expect("terminal event should project");
        assert!(!terminal.done);
        assert_eq!(
            terminal
                .value
                .expect("terminal value should be present")
                .event_type,
            "agent_end"
        );

        let exhausted = stream
            .next()
            .await
            .expect("stream should report exhaustion");
        assert!(exhausted.done);
        assert!(exhausted.value.is_none());
    }

    #[tokio::test]
    async fn terminal_event_waits_for_stream_lifecycle() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(RustAgentEvent::End {
            text: "done".into(),
            usage: a3s_code_core::TokenUsage::default(),
            verification_summary: Box::new(RustVerificationSummary::from_reports(&[])),
            meta: None,
        })
        .await
        .expect("terminal event should enter the test stream");
        drop(tx);

        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let lifecycle = tokio::spawn(async move {
            let _ = release_rx.await;
        });
        let stream = EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(Some(lifecycle))),
        };

        let terminal = tokio::spawn(async move { stream.next().await });
        tokio::task::yield_now().await;
        assert!(
            !terminal.is_finished(),
            "terminal event must not outrun the core stream lifecycle"
        );
        let _ = release_tx.send(());
        let terminal = terminal
            .await
            .expect("terminal projection task should join")
            .expect("terminal event should project");
        assert_eq!(
            terminal
                .value
                .expect("terminal value should be present")
                .event_type,
            "agent_end"
        );
    }
}
