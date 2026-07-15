use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::tools::{Tool, ToolContext, ToolOutput};

use super::{
    code_intelligence_error, format, invalid_argument, provider, query_capabilities,
    structured_success, unavailable,
};

pub(super) struct CodeDiagnosticsTool;

#[async_trait]
impl Tool for CodeDiagnosticsTool {
    fn name(&self) -> &str {
        "code_diagnostics"
    }

    fn description(&self) -> &str {
        "Return language diagnostics for saved workspace code. Provide path for one document or omit it for a bounded manifest-backed workspace query. For document queries only, an empty items array means diagnostics were received and cleared. Workspace queries start relevant language runtimes; truncated can also mean partial language coverage."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Optional saved workspace document path. Omit for workspace diagnostics."
                }
            },
            "examples": [{}, {"path": "src/lib.rs"}]
        })
    }

    fn capabilities(&self, _args: &Value) -> crate::tools::ToolCapabilities {
        query_capabilities()
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = match args.get("path") {
            Some(Value::String(path)) if !path.trim().is_empty() => {
                match ctx.resolve_workspace_path(path) {
                    Ok(path) => Some(path),
                    Err(error) => {
                        return Ok(invalid_argument(format!(
                            "failed to resolve workspace path: {error}"
                        )))
                    }
                }
            }
            Some(Value::String(_)) => return Ok(invalid_argument("path must not be empty")),
            Some(_) => return Ok(invalid_argument("path must be a string")),
            None => None,
        };
        let Some(provider) = provider(ctx) else {
            return Ok(unavailable("diagnostics"));
        };
        match provider
            .diagnostics(path.as_ref(), ctx.cancellation_token())
            .await
        {
            Ok(result) => Ok(structured_success(format::diagnostics(result))),
            Err(error) => Ok(code_intelligence_error("diagnostics", error)),
        }
    }
}
