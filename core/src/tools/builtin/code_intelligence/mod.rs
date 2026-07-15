//! Read-only Code Intelligence tools backed by workspace services.

mod diagnostics;
mod format;
mod navigation;
mod symbols;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::code_intelligence::{CodeIntelligenceError, WorkspaceCodeIntelligence};
use crate::tools::{
    ToolCapabilities, ToolContext, ToolErrorKind, ToolOutput, ToolOutputKind, ToolRegistry,
};

pub(super) fn register(registry: &ToolRegistry) {
    registry.register_builtin(Arc::new(symbols::CodeSymbolsTool));
    registry.register_builtin(Arc::new(navigation::CodeNavigationTool));
    registry.register_builtin(Arc::new(diagnostics::CodeDiagnosticsTool));
}

fn provider(ctx: &ToolContext) -> Option<Arc<dyn WorkspaceCodeIntelligence>> {
    ctx.workspace_services.code_intelligence()
}

fn unavailable(operation: &str) -> ToolOutput {
    code_intelligence_error(
        operation,
        CodeIntelligenceError::Unavailable {
            message: "this workspace did not provide a Code Intelligence runtime".to_owned(),
        },
    )
}

fn invalid_argument(message: impl Into<String>) -> ToolOutput {
    let message = message.into();
    ToolOutput::error(message.clone()).with_error_kind(ToolErrorKind::InvalidArgument { message })
}

fn code_intelligence_error(operation: &str, error: CodeIntelligenceError) -> ToolOutput {
    let error_kind = match &error {
        CodeIntelligenceError::Unsupported { message, .. } => Some(ToolErrorKind::Unsupported {
            message: message.clone(),
        }),
        CodeIntelligenceError::InvalidPath { .. }
        | CodeIntelligenceError::InvalidPosition { .. } => Some(ToolErrorKind::InvalidArgument {
            message: error.to_string(),
        }),
        CodeIntelligenceError::Cancelled => Some(ToolErrorKind::Cancelled {
            op: operation.to_owned(),
        }),
        CodeIntelligenceError::Timeout { duration, .. } => Some(ToolErrorKind::Timeout {
            op: operation.to_owned(),
            duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
        }),
        _ => None,
    };
    let mut output = ToolOutput::error(format!("Code Intelligence query failed: {error}"))
        .with_metadata(serde_json::json!({
            "code": error.code(),
            "operation": operation,
        }));
    if let Some(error_kind) = error_kind {
        output = output.with_error_kind(error_kind);
    }
    output
}

fn structured_success(value: serde_json::Value) -> ToolOutput {
    match serde_json::to_string_pretty(&value) {
        Ok(content) => ToolOutput::success(content).with_metadata(value),
        Err(error) => ToolOutput::error(format!(
            "Code Intelligence result could not be serialized: {error}"
        )),
    }
}

fn query_capabilities() -> ToolCapabilities {
    ToolCapabilities {
        output_kind: ToolOutputKind::Structured,
        ..ToolCapabilities::parallel_safe_read(8)
    }
}
