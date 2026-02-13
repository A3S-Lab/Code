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
use super::MAX_OUTPUT_SIZE;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

/// Substitute `${arg_name}` placeholders in a template with values from a JSON object.
///
/// Shared by BinaryTool and HttpTool to avoid duplicating substitution logic.
pub(crate) fn substitute_template_args(template: &str, args: &serde_json::Value) -> String {
    let mut result = template.to_string();

    if let Some(obj) = args.as_object() {
        for (key, value) in obj {
            let placeholder = format!("${{{}}}", key);
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => value.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }

    result
}

/// Read stdout/stderr from a child process with a size limit and timeout.
///
/// Returns `(output, timed_out)`. If `timed_out` is true, the child is killed.
/// Shared by BinaryTool and ScriptTool to avoid duplicating the select loop.
pub(crate) async fn read_process_output(
    child: &mut Child,
    timeout_secs: u64,
) -> (String, bool) {
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut output = String::new();
    let mut total_size = 0usize;

    let timeout = tokio::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if total_size < MAX_OUTPUT_SIZE {
                                output.push_str(&line);
                                output.push('\n');
                                total_size += line.len() + 1;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if total_size < MAX_OUTPUT_SIZE {
                                output.push_str(&line);
                                output.push('\n');
                                total_size += line.len() + 1;
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
            }
        }
    })
    .await;

    if result.is_err() {
        child.kill().await.ok();
        return (output, true);
    }

    (output, false)
}

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
            Err(CreateToolError::BuiltinNotAllowed)
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
