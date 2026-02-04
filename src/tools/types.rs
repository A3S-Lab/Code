//! Core types for the extensible tool system

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Tool execution context
///
/// Provides tools with access to workspace and other runtime information.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Workspace root directory (sandbox boundary)
    pub workspace: PathBuf,
}

impl ToolContext {
    pub fn new(workspace: PathBuf) -> Self {
        // Canonicalize workspace to handle symlinks (e.g., /var -> /private/var on macOS)
        let canonical_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());
        Self {
            workspace: canonical_workspace,
        }
    }

    /// Resolve path relative to workspace, ensuring it stays within sandbox
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };

        // Canonicalize to resolve .. and symlinks
        let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());

        // Security check: ensure path is within workspace
        if !canonical.starts_with(&self.workspace) {
            anyhow::bail!(
                "Path {} is outside workspace {}",
                canonical.display(),
                self.workspace.display()
            );
        }

        Ok(resolved)
    }

    /// Resolve path for writing (allows non-existent files)
    pub fn resolve_path_for_write(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };

        // For write operations, we can't canonicalize non-existent paths
        // Instead, check that the parent directory is within workspace
        if let Some(parent) = resolved.parent() {
            let canonical_parent = parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf());

            // Allow if parent is workspace or within workspace
            if canonical_parent != self.workspace && !canonical_parent.starts_with(&self.workspace)
            {
                anyhow::bail!(
                    "Path {} is outside workspace {}",
                    resolved.display(),
                    self.workspace.display()
                );
            }
        }

        Ok(resolved)
    }
}

/// Tool execution output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content (text or base64 for binary)
    pub content: String,
    /// Whether execution was successful
    pub success: bool,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            metadata: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            success: false,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Tool trait - the core abstraction for all tools
///
/// Implement this trait to create custom tools that can be registered
/// with the ToolRegistry.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must be unique within registry)
    fn name(&self) -> &str;

    /// Human-readable description for LLM
    fn description(&self) -> &str;

    /// JSON Schema for tool parameters
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool with given arguments
    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput>;
}

/// Tool backend type for dynamic tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolBackend {
    /// Built-in Rust implementation
    #[default]
    Builtin,

    /// External binary executable
    Binary {
        /// URL to download the binary (optional, for skill-based tools)
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Local path to the binary
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Arguments template (use ${arg_name} for substitution)
        #[serde(skip_serializing_if = "Option::is_none")]
        args_template: Option<String>,
    },

    /// HTTP API call
    Http {
        /// API endpoint URL
        url: String,
        /// HTTP method (GET, POST, etc.)
        #[serde(default = "default_http_method")]
        method: String,
        /// Request headers
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        /// Request body template (JSON with ${arg_name} substitution)
        #[serde(skip_serializing_if = "Option::is_none")]
        body_template: Option<String>,
        /// Timeout in milliseconds
        #[serde(default = "default_http_timeout")]
        timeout_ms: u64,
    },

    /// Script execution
    Script {
        /// Interpreter (bash, python, node, etc.)
        interpreter: String,
        /// Script content
        script: String,
        /// Additional interpreter arguments
        #[serde(default)]
        interpreter_args: Vec<String>,
    },
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn default_http_timeout() -> u64 {
    30_000 // 30 seconds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_context_resolve_path() {
        let ctx = ToolContext::new(PathBuf::from("/tmp/workspace"));

        // Relative path
        let resolved = ctx.resolve_path("file.txt");
        assert!(resolved.is_ok());

        // Absolute path within workspace would need the directory to exist
        // so we skip that test
    }

    #[test]
    fn test_tool_output_success() {
        let output = ToolOutput::success("Hello");
        assert!(output.success);
        assert_eq!(output.content, "Hello");
    }

    #[test]
    fn test_tool_output_error() {
        let output = ToolOutput::error("Failed");
        assert!(!output.success);
        assert_eq!(output.content, "Failed");
    }

    #[test]
    fn test_tool_backend_serde() {
        let backend = ToolBackend::Http {
            url: "https://api.example.com".to_string(),
            method: "POST".to_string(),
            headers: std::collections::HashMap::new(),
            body_template: None,
            timeout_ms: 30_000,
        };

        let json = serde_json::to_string(&backend).unwrap();
        assert!(json.contains("http"));
        assert!(json.contains("api.example.com"));
    }
}
