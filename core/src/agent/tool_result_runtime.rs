use super::execution_state::ExecutionLoopState;
use super::AgentLoop;
use crate::llm::{Attachment, Message};
use crate::tools::{ToolErrorKind, ToolResult};
use crate::verification::VerificationReport;
use serde_json::Value;

pub(super) struct NormalizedToolResult {
    pub(super) output: String,
    pub(super) exit_code: i32,
    pub(super) is_error: bool,
    pub(super) metadata: Option<Value>,
    pub(super) images: Vec<Attachment>,
    pub(super) error_kind: Option<ToolErrorKind>,
}

impl NormalizedToolResult {
    pub(super) fn from_execution(result: anyhow::Result<ToolResult>) -> Self {
        match result {
            Ok(result) => Self {
                output: result.output,
                exit_code: result.exit_code,
                is_error: result.exit_code != 0,
                metadata: result.metadata,
                images: result.images,
                error_kind: result.error_kind,
            },
            Err(error) => Self::tool_error(error.to_string()),
        }
    }

    pub(super) fn denied(output: String) -> Self {
        Self {
            output,
            exit_code: 1,
            is_error: true,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }

    pub(super) fn invalid_arguments(tool_name: &str, message: String) -> Self {
        Self {
            output: format!(
                "Invalid arguments for tool '{}': {} [permanent - change the arguments before retrying]",
                tool_name, message
            ),
            exit_code: 1,
            is_error: true,
            metadata: None,
            images: Vec::new(),
            error_kind: Some(ToolErrorKind::InvalidArgument { message }),
        }
    }

    pub(super) fn from_tool_result(result: ToolResult) -> Self {
        Self {
            output: result.output,
            exit_code: result.exit_code,
            is_error: result.exit_code != 0,
            metadata: result.metadata,
            images: result.images,
            error_kind: result.error_kind,
        }
    }

    pub(super) fn into_tool_result(self, name: impl Into<String>) -> ToolResult {
        ToolResult {
            name: name.into(),
            output: self.output,
            exit_code: self.exit_code,
            metadata: self.metadata,
            images: self.images,
            error_kind: self.error_kind,
        }
    }

    fn tool_error(message: String) -> Self {
        let hint = if AgentLoop::is_transient_tool_error(&message) {
            " [transient - you may retry this tool call]"
        } else {
            " [permanent - do not retry without changing the arguments]"
        };

        Self {
            output: format!("Tool execution error: {}{}", message, hint),
            exit_code: 1,
            is_error: true,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }
}

impl AgentLoop {
    pub(super) fn collect_verification_report(
        reports: &mut Vec<VerificationReport>,
        metadata: &Option<Value>,
    ) {
        let Some(metadata) = metadata else {
            return;
        };
        let Some(report) = metadata.get("verification_report") else {
            return;
        };

        match serde_json::from_value::<VerificationReport>(report.clone()) {
            Ok(report) => reports.push(report),
            Err(err) => tracing::warn!(
                error = %err,
                "Ignoring malformed verification_report tool metadata"
            ),
        }
    }

    /// Returns `true` for errors that are likely transient (network, timeout, I/O contention).
    pub(super) fn is_transient_tool_error(message: &str) -> bool {
        let lower = message.to_lowercase();
        lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("connection refused")
            || lower.contains("connection reset")
            || lower.contains("broken pipe")
            || lower.contains("temporarily unavailable")
            || lower.contains("resource temporarily unavailable")
            || lower.contains("os error 11")
            || lower.contains("os error 35")
            || lower.contains("rate limit")
            || lower.contains("too many requests")
            || lower.contains("service unavailable")
            || lower.contains("network unreachable")
    }
}

pub(super) fn push_tool_result_message(
    state: &mut ExecutionLoopState,
    tool_id: &str,
    output: &str,
    is_error: bool,
    images: Vec<Attachment>,
) {
    if images.is_empty() {
        state
            .messages
            .push(Message::tool_result(tool_id, output, is_error));
    } else {
        state.messages.push(Message::tool_result_with_images(
            tool_id, output, &images, is_error,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_tool_error_detection_marks_retryable_failures() {
        assert!(AgentLoop::is_transient_tool_error("request timed out"));
        assert!(AgentLoop::is_transient_tool_error("rate limit exceeded"));
        assert!(!AgentLoop::is_transient_tool_error("unknown argument path"));
    }

    #[test]
    fn execution_error_includes_retry_hint() {
        let transient = NormalizedToolResult::from_execution(Err(anyhow::anyhow!("timed out")));
        assert!(transient.output.contains("transient"));

        let permanent = NormalizedToolResult::from_execution(Err(anyhow::anyhow!("bad path")));
        assert!(permanent.output.contains("permanent"));
    }
}
