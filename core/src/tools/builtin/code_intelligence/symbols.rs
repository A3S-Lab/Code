use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::tools::{Tool, ToolContext, ToolOutput};

use super::{
    code_intelligence_error, format, invalid_argument, provider, query_capabilities,
    structured_success, unavailable,
};

const DEFAULT_SEARCH_LIMIT: usize = 100;
const MAX_SEARCH_LIMIT: usize = 500;

pub(super) struct CodeSymbolsTool;

#[async_trait]
impl Tool for CodeSymbolsTool {
    fn name(&self) -> &str {
        "code_symbols"
    }

    fn description(&self) -> &str {
        "Query semantic symbols from saved workspace files. Use operation='outline' for a document hierarchy or operation='search' for workspace symbols. Returns symbol metadata and locations, not source code; use read when source text is needed."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["outline", "search"]
                },
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Saved workspace document path. Required for outline."
                },
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Semantic symbol query. Required for search."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SEARCH_LIMIT,
                    "description": "Maximum workspace symbols for search. Default: 100; maximum: 500."
                }
            },
            "required": ["operation"],
            "oneOf": [
                {
                    "properties": {"operation": {"const": "outline"}},
                    "required": ["path"]
                },
                {
                    "properties": {"operation": {"const": "search"}},
                    "required": ["query"]
                }
            ],
            "examples": [
                {"operation": "outline", "path": "src/lib.rs"},
                {"operation": "search", "query": "WorkspaceClient", "limit": 50}
            ]
        })
    }

    fn capabilities(&self, _args: &Value) -> crate::tools::ToolCapabilities {
        query_capabilities()
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let operation = match args.get("operation").and_then(Value::as_str) {
            Some(operation @ ("outline" | "search")) => operation,
            Some(_) => return Ok(invalid_argument("operation must be 'outline' or 'search'")),
            None => return Ok(invalid_argument("operation parameter is required")),
        };
        let Some(provider) = provider(ctx) else {
            return Ok(unavailable(operation));
        };

        if operation == "outline" {
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                return Ok(invalid_argument("path is required for outline"));
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
            match provider
                .document_symbols(&path, ctx.cancellation_token())
                .await
            {
                Ok(result) => Ok(structured_success(format::document_symbols(result))),
                Err(error) => Ok(code_intelligence_error(operation, error)),
            }
        } else {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return Ok(invalid_argument("query is required for search"));
            };
            let query = query.trim();
            if query.is_empty() {
                return Ok(invalid_argument("query must not be empty"));
            }
            let limit = match args.get("limit") {
                Some(limit) => match limit.as_u64().and_then(|limit| usize::try_from(limit).ok()) {
                    Some(limit) if (1..=MAX_SEARCH_LIMIT).contains(&limit) => limit,
                    _ => {
                        return Ok(invalid_argument(format!(
                            "limit must be an integer between 1 and {MAX_SEARCH_LIMIT}"
                        )))
                    }
                },
                None => DEFAULT_SEARCH_LIMIT,
            };
            match provider
                .search_symbols(query, limit, ctx.cancellation_token())
                .await
            {
                Ok(result) => Ok(structured_success(format::workspace_symbols(result))),
                Err(error) => Ok(code_intelligence_error(operation, error)),
            }
        }
    }
}
