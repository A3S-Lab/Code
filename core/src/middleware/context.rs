//! Middleware execution context
//!
//! Provides the context object passed through the middleware pipeline,
//! similar to Express's req/res objects.

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// Tool call information in middleware context
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// Middleware execution context
///
/// This is the context object passed through the middleware pipeline.
/// Middleware can read from and write to this context to share state
/// and modify the execution flow.
#[derive(Debug, Clone)]
pub struct MiddlewareContext {
    /// Session ID
    pub session_id: String,

    /// Workspace directory
    pub workspace: PathBuf,

    /// User prompt (if this is a prompt event)
    pub prompt: Option<String>,

    /// Tool call (if this is a tool execution event)
    pub tool_call: Option<ToolCallInfo>,

    /// Arbitrary metadata that middleware can read/write
    pub metadata: HashMap<String, Value>,
}

impl MiddlewareContext {
    /// Create a new middleware context
    pub fn new(session_id: String, workspace: PathBuf) -> Self {
        Self {
            session_id,
            workspace,
            prompt: None,
            tool_call: None,
            metadata: HashMap::new(),
        }
    }

    /// Set prompt
    pub fn with_prompt(mut self, prompt: String) -> Self {
        self.prompt = Some(prompt);
        self
    }

    /// Set tool call
    pub fn with_tool_call(mut self, tool_call: ToolCallInfo) -> Self {
        self.tool_call = Some(tool_call);
        self
    }

    /// Get metadata value
    pub fn get_metadata(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }

    /// Set metadata value
    pub fn set_metadata(&mut self, key: String, value: Value) {
        self.metadata.insert(key, value);
    }
}
