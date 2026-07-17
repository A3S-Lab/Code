//! Stateful sanitization for streamed agent event fields.

use super::{sanitize_agent_event, SecurityProvider};
use std::collections::HashMap;
use std::sync::Arc;

/// Stateful sanitizer for streamed agent events.
///
/// A [`SecurityProvider`] has no contract that bounds the length of a secret
/// recognized by [`SecurityProvider::sanitize_output`]. Consequently, no fixed
/// overlap between adjacent deltas can make per-delta sanitization safe. When a
/// provider is configured, this type buffers each consumer-visible
/// concatenation domain and sanitizes the complete value at its semantic
/// boundary. Security-enabled streams therefore intentionally trade token-level
/// latency for a boundary that cannot be bypassed by splitting a secret across
/// chunks.
pub(crate) struct AgentEventStreamSanitizer {
    provider: Option<Arc<dyn SecurityProvider>>,
    next_sequence: u64,
    buffered_bytes: usize,
    failed_closed: bool,
    text: Option<BufferedDelta>,
    reasoning: Option<BufferedDelta>,
    tool_inputs: HashMap<ToolInputKey, BufferedToolInput>,
    tool_outputs: HashMap<String, BufferedToolOutput>,
    current_tool_input_id: Option<String>,
    active_turn: Option<usize>,
    attempt_checkpoint: Option<StreamAttemptCheckpoint>,
}

#[derive(Clone)]
struct BufferedDelta {
    sequence: u64,
    value: String,
}

#[derive(Clone)]
struct BufferedToolInput {
    sequence: u64,
    emitted_id: Option<String>,
    value: String,
}

#[derive(Clone)]
struct BufferedToolOutput {
    sequence: u64,
    name: String,
    value: String,
}

#[derive(Clone)]
struct StreamAttemptCheckpoint {
    next_sequence: u64,
    buffered_bytes: usize,
    failed_closed: bool,
    text: Option<BufferedDelta>,
    reasoning: Option<BufferedDelta>,
    tool_inputs: HashMap<ToolInputKey, BufferedToolInput>,
    tool_outputs: HashMap<String, BufferedToolOutput>,
    current_tool_input_id: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ToolInputKey {
    Known(String),
    Anonymous,
}

// Full-domain buffering is required for arbitrary SecurityProvider matchers,
// but must not let a hostile stream grow memory without bound. Crossing this
// aggregate limit drops all buffered and future deltas for the invocation. It
// never truncates and exposes a prefix, which would reintroduce split-secret
// bypasses.
const MAX_BUFFERED_STREAM_BYTES: usize = 8 * 1024 * 1024;

impl AgentEventStreamSanitizer {
    pub(crate) fn new(provider: Option<Arc<dyn SecurityProvider>>) -> Self {
        Self {
            provider,
            next_sequence: 0,
            buffered_bytes: 0,
            failed_closed: false,
            text: None,
            reasoning: None,
            tool_inputs: HashMap::new(),
            tool_outputs: HashMap::new(),
            current_tool_input_id: None,
            active_turn: None,
            attempt_checkpoint: None,
        }
    }

    /// Accept one event and return the sanitized events whose complete fields
    /// are now safe to expose. A buffered delta may make this return no events.
    pub(crate) fn push(
        &mut self,
        event: crate::agent::AgentEvent,
    ) -> Vec<crate::agent::AgentEvent> {
        use crate::agent::AgentEvent;

        if self.provider.is_none() {
            return vec![event];
        }

        match event {
            AgentEvent::TextDelta { text } => {
                if !self.admit(text.len()) {
                    return Vec::new();
                }
                let sequence = self.reserve_sequence();
                append_delta(&mut self.text, sequence, text);
                Vec::new()
            }
            AgentEvent::ReasoningDelta { text } => {
                if !self.admit(text.len()) {
                    return Vec::new();
                }
                let sequence = self.reserve_sequence();
                append_delta(&mut self.reasoning, sequence, text);
                Vec::new()
            }
            AgentEvent::ToolInputDelta { id, delta } => {
                if let Some(id) = id.as_deref() {
                    self.bind_anonymous_tool_input(id);
                    self.current_tool_input_id = Some(id.to_string());
                }

                let key = id
                    .as_ref()
                    .or(self.current_tool_input_id.as_ref())
                    .map(|id| ToolInputKey::Known(id.clone()))
                    .unwrap_or(ToolInputKey::Anonymous);
                if !self.admit(delta.len()) {
                    return Vec::new();
                }
                let sequence = self.reserve_sequence();
                match self.tool_inputs.get_mut(&key) {
                    Some(buffer) => {
                        if buffer.emitted_id.is_none() {
                            buffer.emitted_id = id;
                        }
                        buffer.value.push_str(&delta);
                    }
                    None => {
                        self.tool_inputs.insert(
                            key,
                            BufferedToolInput {
                                sequence,
                                emitted_id: id,
                                value: delta,
                            },
                        );
                    }
                }
                Vec::new()
            }
            AgentEvent::ToolOutputDelta { id, name, delta } => {
                if !self.admit(delta.len()) {
                    return Vec::new();
                }
                let sequence = self.reserve_sequence();
                match self.tool_outputs.get_mut(&id) {
                    Some(buffer) => buffer.value.push_str(&delta),
                    None => {
                        self.tool_outputs.insert(
                            id,
                            BufferedToolOutput {
                                sequence,
                                name,
                                value: delta,
                            },
                        );
                    }
                }
                Vec::new()
            }
            event => {
                let mut ready = Vec::new();
                match &event {
                    AgentEvent::TurnStart { turn } => self.begin_or_restart_turn(*turn),
                    AgentEvent::ToolStart { id, .. } => {
                        self.bind_anonymous_tool_input(id);
                        self.current_tool_input_id = Some(id.clone());
                    }
                    AgentEvent::ToolExecutionStart { id, .. } => {
                        self.take_tool_fields(id, false, &mut ready);
                    }
                    AgentEvent::ToolEnd { id, .. } => {
                        self.take_tool_fields(id, true, &mut ready);
                    }
                    AgentEvent::PermissionDenied { tool_id, .. }
                    | AgentEvent::ConfirmationRequired { tool_id, .. }
                    | AgentEvent::ConfirmationReceived { tool_id, .. }
                    | AgentEvent::ConfirmationTimeout { tool_id, .. } => {
                        self.take_tool_fields(tool_id, false, &mut ready);
                    }
                    AgentEvent::TurnEnd { .. } => {
                        self.take_all_tool_inputs(&mut ready);
                        self.active_turn = None;
                        self.attempt_checkpoint = None;
                    }
                    AgentEvent::End { .. } | AgentEvent::Error { .. } => {
                        self.take_all(&mut ready);
                        self.active_turn = None;
                        self.attempt_checkpoint = None;
                    }
                    _ => {}
                }

                let mut output = self.sanitize_pending(ready);
                if let Some(event) = self.sanitize(&event) {
                    output.push(event);
                }
                output
            }
        }
    }

    /// Flush buffered fields when a producer closes without a terminal event.
    pub(crate) fn finish(&mut self) -> Vec<crate::agent::AgentEvent> {
        if self.provider.is_none() {
            return Vec::new();
        }
        let mut ready = Vec::new();
        self.take_all(&mut ready);
        self.sanitize_pending(ready)
    }

    /// A repeated `TurnStart` with the same turn number marks an in-place LLM
    /// stream retry. Restore buffered security domains to their state before
    /// that attempt so discarded partial output can never be concatenated with
    /// the replacement response.
    fn begin_or_restart_turn(&mut self, turn: usize) {
        if self.active_turn == Some(turn) {
            if let Some(checkpoint) = self.attempt_checkpoint.clone() {
                self.restore_attempt_checkpoint(checkpoint);
            }
            return;
        }

        self.active_turn = Some(turn);
        self.attempt_checkpoint = Some(self.attempt_checkpoint());
    }

    fn attempt_checkpoint(&self) -> StreamAttemptCheckpoint {
        StreamAttemptCheckpoint {
            next_sequence: self.next_sequence,
            buffered_bytes: self.buffered_bytes,
            failed_closed: self.failed_closed,
            text: self.text.clone(),
            reasoning: self.reasoning.clone(),
            tool_inputs: self.tool_inputs.clone(),
            tool_outputs: self.tool_outputs.clone(),
            current_tool_input_id: self.current_tool_input_id.clone(),
        }
    }

    fn restore_attempt_checkpoint(&mut self, checkpoint: StreamAttemptCheckpoint) {
        self.next_sequence = checkpoint.next_sequence;
        self.buffered_bytes = checkpoint.buffered_bytes;
        self.failed_closed = checkpoint.failed_closed;
        self.text = checkpoint.text;
        self.reasoning = checkpoint.reasoning;
        self.tool_inputs = checkpoint.tool_inputs;
        self.tool_outputs = checkpoint.tool_outputs;
        self.current_tool_input_id = checkpoint.current_tool_input_id;
    }

    fn reserve_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        sequence
    }

    fn sanitize(&self, event: &crate::agent::AgentEvent) -> Option<crate::agent::AgentEvent> {
        self.provider
            .as_deref()
            .map(|provider| sanitize_agent_event(provider, event))
    }

    fn sanitize_pending(
        &self,
        mut pending: Vec<(u64, crate::agent::AgentEvent)>,
    ) -> Vec<crate::agent::AgentEvent> {
        pending.sort_by_key(|(sequence, _)| *sequence);
        pending
            .into_iter()
            .filter_map(|(_, event)| self.sanitize(&event))
            .collect()
    }

    fn take_tool_fields(
        &mut self,
        id: &str,
        include_output: bool,
        ready: &mut Vec<(u64, crate::agent::AgentEvent)>,
    ) {
        self.take_tool_input(&ToolInputKey::Known(id.to_string()), ready);
        if self.current_tool_input_id.as_deref() == Some(id) {
            self.current_tool_input_id = None;
        }
        if include_output {
            self.take_tool_output(id, ready);
        }
    }

    fn take_tool_input(
        &mut self,
        key: &ToolInputKey,
        ready: &mut Vec<(u64, crate::agent::AgentEvent)>,
    ) {
        if let Some(buffer) = self.tool_inputs.remove(key) {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::ToolInputDelta {
                    id: buffer.emitted_id,
                    delta: buffer.value,
                },
            ));
        }
    }

    fn take_tool_output(&mut self, id: &str, ready: &mut Vec<(u64, crate::agent::AgentEvent)>) {
        if let Some(buffer) = self.tool_outputs.remove(id) {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::ToolOutputDelta {
                    id: id.to_string(),
                    name: buffer.name,
                    delta: buffer.value,
                },
            ));
        }
    }

    fn take_all_tool_inputs(&mut self, ready: &mut Vec<(u64, crate::agent::AgentEvent)>) {
        for (_, buffer) in self.tool_inputs.drain() {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::ToolInputDelta {
                    id: buffer.emitted_id,
                    delta: buffer.value,
                },
            ));
        }
        self.current_tool_input_id = None;
    }

    fn take_all(&mut self, ready: &mut Vec<(u64, crate::agent::AgentEvent)>) {
        if let Some(buffer) = self.text.take() {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::TextDelta { text: buffer.value },
            ));
        }
        if let Some(buffer) = self.reasoning.take() {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::ReasoningDelta { text: buffer.value },
            ));
        }
        self.take_all_tool_inputs(ready);
        for (id, buffer) in self.tool_outputs.drain() {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(buffer.value.len());
            ready.push((
                buffer.sequence,
                crate::agent::AgentEvent::ToolOutputDelta {
                    id,
                    name: buffer.name,
                    delta: buffer.value,
                },
            ));
        }
    }

    fn bind_anonymous_tool_input(&mut self, id: &str) {
        let Some(anonymous) = self.tool_inputs.remove(&ToolInputKey::Anonymous) else {
            return;
        };
        let known_key = ToolInputKey::Known(id.to_string());
        match self.tool_inputs.remove(&known_key) {
            Some(known) if known.sequence < anonymous.sequence => {
                self.tool_inputs.insert(
                    known_key,
                    BufferedToolInput {
                        sequence: known.sequence,
                        emitted_id: Some(id.to_string()),
                        value: known.value + &anonymous.value,
                    },
                );
            }
            Some(known) => {
                self.tool_inputs.insert(
                    known_key,
                    BufferedToolInput {
                        sequence: anonymous.sequence,
                        emitted_id: Some(id.to_string()),
                        value: anonymous.value + &known.value,
                    },
                );
            }
            None => {
                self.tool_inputs.insert(
                    known_key,
                    BufferedToolInput {
                        sequence: anonymous.sequence,
                        emitted_id: Some(id.to_string()),
                        value: anonymous.value,
                    },
                );
            }
        }
    }

    fn admit(&mut self, additional_bytes: usize) -> bool {
        if self.failed_closed {
            return false;
        }
        let Some(total) = self.buffered_bytes.checked_add(additional_bytes) else {
            self.fail_closed();
            return false;
        };
        if total > MAX_BUFFERED_STREAM_BYTES {
            self.fail_closed();
            return false;
        }
        self.buffered_bytes = total;
        true
    }

    fn fail_closed(&mut self) {
        self.failed_closed = true;
        self.buffered_bytes = 0;
        self.text = None;
        self.reasoning = None;
        self.tool_inputs.clear();
        self.tool_outputs.clear();
        self.current_tool_input_id = None;
        tracing::warn!(
            limit_bytes = MAX_BUFFERED_STREAM_BYTES,
            "security stream buffer exceeded its limit; dropping all deltas for this invocation"
        );
    }
}

fn append_delta(buffer: &mut Option<BufferedDelta>, sequence: u64, value: String) {
    match buffer {
        Some(buffer) => buffer.value.push_str(&value),
        None => *buffer = Some(BufferedDelta { sequence, value }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::DefaultSecurityProvider;

    #[test]
    fn stream_sanitizer_redacts_all_delta_kinds_across_chunk_boundaries() {
        use crate::agent::AgentEvent;

        let provider: Arc<dyn SecurityProvider> = Arc::new(DefaultSecurityProvider::new());
        let mut sanitizer = AgentEventStreamSanitizer::new(Some(provider));
        let mut emitted = Vec::new();

        for event in [
            AgentEvent::TextDelta {
                text: "text@".to_string(),
            },
            AgentEvent::TextDelta {
                text: "example.com".to_string(),
            },
            AgentEvent::ReasoningDelta {
                text: "reasoning@".to_string(),
            },
            AgentEvent::ReasoningDelta {
                text: "example.com".to_string(),
            },
            AgentEvent::ToolStart {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
            },
            AgentEvent::ToolInputDelta {
                id: Some("tool-1".to_string()),
                delta: r#"{"contact":"input@"#.to_string(),
            },
            AgentEvent::ToolInputDelta {
                id: Some("tool-1".to_string()),
                delta: r#"example.com"}"#.to_string(),
            },
            AgentEvent::ToolExecutionStart {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({}),
            },
            AgentEvent::ToolOutputDelta {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                delta: "output@".to_string(),
            },
            AgentEvent::ToolOutputDelta {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                delta: "example.com".to_string(),
            },
            AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                args: Some(serde_json::json!({})),
                output: "done".to_string(),
                exit_code: 0,
                metadata: None,
                error_kind: None,
            },
            AgentEvent::End {
                text: "done".to_string(),
                usage: crate::llm::TokenUsage::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                meta: None,
            },
        ] {
            emitted.extend(sanitizer.push(event));
        }

        assert!(sanitizer.finish().is_empty());
        let serialized = emitted
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        for secret in [
            "text@example.com",
            "reasoning@example.com",
            "input@example.com",
            "output@example.com",
        ] {
            assert!(!serialized.contains(secret), "unsanitized secret: {secret}");
        }
        assert_eq!(serialized.matches("REDACTED:EMAIL").count(), 4);

        let input = emitted
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolInputDelta { .. }))
            .unwrap();
        let execution_start = emitted
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .unwrap();
        let output = emitted
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolOutputDelta { .. }))
            .unwrap();
        let tool_end = emitted
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolEnd { .. }))
            .unwrap();
        let end = emitted
            .iter()
            .position(|event| matches!(event, AgentEvent::End { .. }))
            .unwrap();
        assert!(input < execution_start);
        assert!(output < tool_end);
        assert!(emitted[..end]
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { .. })));
        assert!(emitted[..end]
            .iter()
            .any(|event| matches!(event, AgentEvent::ReasoningDelta { .. })));
    }

    #[test]
    fn tool_input_buffers_survive_interleaving_and_bind_anonymous_prefixes() {
        use crate::agent::AgentEvent;

        let provider: Arc<dyn SecurityProvider> = Arc::new(DefaultSecurityProvider::new());
        let mut sanitizer = AgentEventStreamSanitizer::new(Some(Arc::clone(&provider)));

        assert_eq!(
            sanitizer
                .push(AgentEvent::ToolInputDelta {
                    id: Some("a".to_string()),
                    delta: "alice@".to_string(),
                })
                .len(),
            0
        );
        sanitizer.push(AgentEvent::ToolInputDelta {
            id: Some("b".to_string()),
            delta: "safe".to_string(),
        });
        sanitizer.push(AgentEvent::ToolInputDelta {
            id: Some("a".to_string()),
            delta: "example.com".to_string(),
        });
        let ready = sanitizer.push(AgentEvent::ToolExecutionStart {
            id: "a".to_string(),
            name: "test".to_string(),
            args: serde_json::json!({}),
        });
        assert!(matches!(
            &ready[0],
            AgentEvent::ToolInputDelta { id: Some(id), delta }
                if id == "a" && delta.contains("REDACTED:EMAIL")
        ));
        assert!(!serde_json::to_string(&ready)
            .unwrap()
            .contains("alice@example.com"));

        let mut anonymous = AgentEventStreamSanitizer::new(Some(provider));
        anonymous.push(AgentEvent::ToolInputDelta {
            id: None,
            delta: "anonymous@".to_string(),
        });
        anonymous.push(AgentEvent::ToolStart {
            id: "bound".to_string(),
            name: "test".to_string(),
        });
        anonymous.push(AgentEvent::ToolInputDelta {
            id: None,
            delta: "example.com".to_string(),
        });
        let ready = anonymous.push(AgentEvent::ToolExecutionStart {
            id: "bound".to_string(),
            name: "test".to_string(),
            args: serde_json::json!({}),
        });
        assert!(matches!(
            &ready[0],
            AgentEvent::ToolInputDelta { id: Some(id), delta }
                if id == "bound" && delta.contains("REDACTED:EMAIL")
        ));
        assert!(!serde_json::to_string(&ready)
            .unwrap()
            .contains("anonymous@example.com"));
    }

    #[test]
    fn stream_sanitizer_makes_no_custom_match_length_overlap_assumption() {
        struct ExactSecret(String);

        impl SecurityProvider for ExactSecret {
            fn sanitize_output(&self, text: &str) -> String {
                text.replace(&self.0, "[REDACTED:CUSTOM]")
            }
        }

        let secret = format!("begin-{}-end", "x".repeat(32 * 1024));
        let split = secret.len() / 2;
        let provider: Arc<dyn SecurityProvider> = Arc::new(ExactSecret(secret.clone()));
        let mut sanitizer = AgentEventStreamSanitizer::new(Some(provider));
        assert!(sanitizer
            .push(crate::agent::AgentEvent::TextDelta {
                text: secret[..split].to_string(),
            })
            .is_empty());
        assert!(sanitizer
            .push(crate::agent::AgentEvent::TextDelta {
                text: secret[split..].to_string(),
            })
            .is_empty());
        let emitted = sanitizer.push(crate::agent::AgentEvent::Error {
            message: "terminal".to_string(),
        });
        assert!(matches!(
            &emitted[0],
            crate::agent::AgentEvent::TextDelta { text }
                if text == "[REDACTED:CUSTOM]"
        ));
        assert!(matches!(
            emitted.last(),
            Some(crate::agent::AgentEvent::Error { .. })
        ));
    }

    #[test]
    fn repeated_turn_start_discards_only_the_interrupted_attempt_buffers() {
        use crate::agent::AgentEvent;

        let provider: Arc<dyn SecurityProvider> = Arc::new(DefaultSecurityProvider::new());
        let mut sanitizer = AgentEventStreamSanitizer::new(Some(provider));
        let mut emitted = Vec::new();

        emitted.extend(sanitizer.push(AgentEvent::TurnStart { turn: 1 }));
        emitted.extend(sanitizer.push(AgentEvent::TextDelta {
            text: "stable response. ".to_string(),
        }));
        emitted.extend(sanitizer.push(AgentEvent::TurnEnd {
            turn: 1,
            usage: crate::llm::TokenUsage::default(),
        }));

        emitted.extend(sanitizer.push(AgentEvent::TurnStart { turn: 2 }));
        emitted.extend(sanitizer.push(AgentEvent::TextDelta {
            text: "discarded partial. ".to_string(),
        }));
        emitted.extend(sanitizer.push(AgentEvent::ReasoningDelta {
            text: "discarded reasoning. ".to_string(),
        }));
        emitted.extend(sanitizer.push(AgentEvent::ToolStart {
            id: "discarded-tool".to_string(),
            name: "bash".to_string(),
        }));
        emitted.extend(sanitizer.push(AgentEvent::ToolInputDelta {
            id: Some("discarded-tool".to_string()),
            delta: "discarded input".to_string(),
        }));

        // Core re-emits the same turn number immediately before waiting and
        // restarting the provider request.
        emitted.extend(sanitizer.push(AgentEvent::TurnStart { turn: 2 }));
        emitted.extend(sanitizer.push(AgentEvent::TextDelta {
            text: "recovered response.".to_string(),
        }));
        emitted.extend(sanitizer.push(AgentEvent::End {
            text: "recovered response.".to_string(),
            usage: crate::llm::TokenUsage::default(),
            verification_summary: Box::new(crate::verification::VerificationSummary::from_reports(
                &[],
            )),
            meta: None,
        }));

        let serialized = serde_json::to_string(&emitted).unwrap();
        assert!(serialized.contains("stable response"), "{serialized}");
        assert!(serialized.contains("recovered response"), "{serialized}");
        assert!(!serialized.contains("discarded partial"), "{serialized}");
        assert!(!serialized.contains("discarded reasoning"), "{serialized}");
        assert!(!serialized.contains("discarded input"), "{serialized}");
    }

    #[test]
    fn stream_sanitizer_drops_all_deltas_after_aggregate_limit() {
        let provider: Arc<dyn SecurityProvider> = Arc::new(DefaultSecurityProvider::new());
        let mut sanitizer = AgentEventStreamSanitizer::new(Some(provider));
        assert!(sanitizer
            .push(crate::agent::AgentEvent::TextDelta {
                text: "x".repeat(MAX_BUFFERED_STREAM_BYTES),
            })
            .is_empty());
        assert!(sanitizer
            .push(crate::agent::AgentEvent::TextDelta {
                text: "y".to_string(),
            })
            .is_empty());

        let emitted = sanitizer.push(crate::agent::AgentEvent::Error {
            message: "stream redaction buffer exceeded".to_string(),
        });
        assert_eq!(emitted.len(), 1);
        assert!(matches!(
            emitted.first(),
            Some(crate::agent::AgentEvent::Error { .. })
        ));
    }
}
