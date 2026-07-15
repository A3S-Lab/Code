use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::{
    code_intelligence::{CodePosition, NavigationKind},
    tools::{Tool, ToolContext, ToolOutput},
};

use super::{
    code_intelligence_error, format, invalid_argument, provider, query_capabilities,
    structured_success, unavailable,
};

pub(super) struct CodeNavigationTool;

#[async_trait]
impl Tool for CodeNavigationTool {
    fn name(&self) -> &str {
        "code_navigation"
    }

    fn description(&self) -> &str {
        "Navigate saved workspace code by language semantics. Resolves definitions, declarations, references, or implementations from a zero-based UTF-16 position. Returns locations only, not source code."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["definition", "declaration", "references", "implementations"]
                },
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Saved workspace document path."
                },
                "line": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4294967295_u64,
                    "description": "Zero-based line."
                },
                "character": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4294967295_u64,
                    "description": "Zero-based UTF-16 code-unit offset."
                }
            },
            "required": ["operation", "path", "line", "character"],
            "examples": [{
                "operation": "definition",
                "path": "src/lib.rs",
                "line": 12,
                "character": 8
            }]
        })
    }

    fn capabilities(&self, _args: &Value) -> crate::tools::ToolCapabilities {
        query_capabilities()
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let (operation, kind) = match args.get("operation").and_then(Value::as_str) {
            Some("definition") => ("definition", NavigationKind::Definition),
            Some("declaration") => ("declaration", NavigationKind::Declaration),
            Some("references") => ("references", NavigationKind::References),
            Some("implementations") => ("implementations", NavigationKind::Implementations),
            Some(_) => {
                return Ok(invalid_argument(
                    "operation must be definition, declaration, references, or implementations",
                ))
            }
            None => return Ok(invalid_argument("operation parameter is required")),
        };
        let Some(path) = args.get("path").and_then(Value::as_str) else {
            return Ok(invalid_argument("path parameter is required"));
        };
        if path.trim().is_empty() {
            return Ok(invalid_argument("path must not be empty"));
        }
        let path = match ctx.resolve_workspace_path(path) {
            Ok(path) => path,
            Err(error) => {
                return Ok(invalid_argument(format!(
                    "failed to resolve workspace path: {error}"
                )))
            }
        };
        let line = match u32_argument(args, "line") {
            Ok(value) => value,
            Err(message) => return Ok(invalid_argument(message)),
        };
        let character = match u32_argument(args, "character") {
            Ok(value) => value,
            Err(message) => return Ok(invalid_argument(message)),
        };
        let Some(provider) = provider(ctx) else {
            return Ok(unavailable(operation));
        };
        match provider
            .navigate(
                kind,
                &path,
                CodePosition::new(line, character),
                ctx.cancellation_token(),
            )
            .await
        {
            Ok(result) => Ok(structured_success(format::locations(result))),
            Err(error) => Ok(code_intelligence_error(operation, error)),
        }
    }
}

fn u32_argument(args: &Value, name: &str) -> std::result::Result<u32, String> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("{name} must be an integer from 0 to 4294967295"))
}
