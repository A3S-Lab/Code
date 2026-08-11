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
            }
            .with_evidence(),
            Err(error) => Self::tool_error(error.to_string()),
        }
    }

    pub(super) fn denied(output: String) -> Self {
        Self::denied_with_error_kind(output, None)
    }

    pub(super) fn denied_with_error_kind(
        output: String,
        error_kind: Option<ToolErrorKind>,
    ) -> Self {
        Self {
            output,
            exit_code: 1,
            is_error: true,
            metadata: None,
            images: Vec::new(),
            error_kind,
        }
        .with_evidence()
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
        .with_evidence()
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
        .with_evidence()
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
        Self {
            output: format!("Tool execution error: {message}"),
            exit_code: 1,
            is_error: true,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
        .with_evidence()
    }

    fn with_evidence(mut self) -> Self {
        self.metadata = Some(crate::tools::ensure_tool_result_evidence(
            self.metadata,
            &self.output,
        ));
        self
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
    fn untyped_execution_error_prose_cannot_create_retry_guidance() {
        let result = NormalizedToolResult::from_execution(Err(anyhow::anyhow!(
            "timeout, connection reset, rate limit, and too many requests"
        )));

        assert!(!result.output.contains("[transient"));
        assert!(!result.output.contains("[permanent"));
        assert_eq!(result.error_kind, None);
        let evidence = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("a3s_tool_result_evidence"))
            .expect("context evidence");
        assert_eq!(
            evidence["schema"],
            crate::tools::TOOL_RESULT_EVIDENCE_SCHEMA_V1
        );
        assert_eq!(evidence["loss_mode"], "none");
    }
}
