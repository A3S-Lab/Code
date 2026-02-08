//! Dynamic tool implementations
//!
//! These tools are loaded at runtime from skills:
//! - BinaryTool: Execute external binaries
//! - HttpTool: Make HTTP API calls
//! - ScriptTool: Execute scripts with interpreters

mod binary;
mod http;
mod script;

pub use binary::BinaryTool;
pub use http::HttpTool;
pub use script::ScriptTool;

use super::types::ToolBackend;
use super::Tool;
use std::sync::Arc;

/// Error type for dynamic tool creation
#[derive(Debug, thiserror::Error)]
pub enum CreateToolError {
    #[error("Cannot create builtin tool through create_tool() — register builtin tools directly")]
    BuiltinNotAllowed,
}

/// Create a dynamic tool from a backend specification
#[allow(dead_code)]
pub fn create_tool(
    name: String,
    description: String,
    parameters: serde_json::Value,
    backend: ToolBackend,
) -> Result<Arc<dyn Tool>, CreateToolError> {
    match backend {
        ToolBackend::Builtin => {
            // Builtin tools should be registered directly, not through this function
            return Err(CreateToolError::BuiltinNotAllowed);
        }
        ToolBackend::Binary {
            url,
            path,
            args_template,
        } => Ok(Arc::new(BinaryTool::new(
            name,
            description,
            parameters,
            url,
            path,
            args_template,
        ))),
        ToolBackend::Http {
            url,
            method,
            headers,
            body_template,
            timeout_ms,
        } => Ok(Arc::new(HttpTool::new(
            name,
            description,
            parameters,
            url,
            method,
            headers,
            body_template,
            timeout_ms,
        ))),
        ToolBackend::Script {
            interpreter,
            script,
            interpreter_args,
        } => Ok(Arc::new(ScriptTool::new(
            name,
            description,
            parameters,
            interpreter,
            script,
            interpreter_args,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_script_tool() {
        let tool = create_tool(
            "test".to_string(),
            "A test tool".to_string(),
            serde_json::json!({"type": "object", "properties": {}}),
            ToolBackend::Script {
                interpreter: "bash".to_string(),
                script: "echo hello".to_string(),
                interpreter_args: vec![],
            },
        )
        .unwrap();

        assert_eq!(tool.name(), "test");
        assert_eq!(tool.description(), "A test tool");
    }

    #[test]
    fn test_create_http_tool() {
        let tool = create_tool(
            "api".to_string(),
            "An API tool".to_string(),
            serde_json::json!({"type": "object", "properties": {}}),
            ToolBackend::Http {
                url: "https://api.example.com".to_string(),
                method: "POST".to_string(),
                headers: std::collections::HashMap::new(),
                body_template: None,
                timeout_ms: 30_000,
            },
        )
        .unwrap();

        assert_eq!(tool.name(), "api");
    }

    #[test]
    fn test_create_binary_tool() {
        let tool = create_tool(
            "bin".to_string(),
            "A binary tool".to_string(),
            serde_json::json!({"type": "object", "properties": {}}),
            ToolBackend::Binary {
                url: None,
                path: Some("/usr/bin/echo".to_string()),
                args_template: Some("${message}".to_string()),
            },
        )
        .unwrap();

        assert_eq!(tool.name(), "bin");
    }

    #[test]
    fn test_create_builtin_returns_error() {
        let result = create_tool(
            "builtin".to_string(),
            "A builtin tool".to_string(),
            serde_json::json!({}),
            ToolBackend::Builtin,
        );
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Cannot create builtin tool"));
    }
}
