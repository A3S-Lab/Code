//! Agent Facade API
//!
//! High-level, ergonomic API for using A3S Code as an embedded library.
//! Wraps the internal `AgentLoop`, `ToolExecutor`, and `LlmClient` behind
//! a simple builder pattern.
//!
//! ## Example
//!
//! ```rust,no_run
//! use a3s_code_core::Agent;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let agent = Agent::builder()
//!     .model("claude-sonnet-4-20250514")
//!     .api_key("sk-ant-...")
//!     .workspace("/my-project")
//!     .build()
//!     .await?;
//!
//! let result = agent.send("Explain the auth module").await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::hooks::HookEngine;
use crate::llm::{AnthropicClient, LlmClient, OpenAiClient, ToolDefinition};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

/// High-level agent facade for embedded library usage.
///
/// Combines an LLM client, tool executor, and agent loop into a single
/// ergonomic API. Created via [`Agent::builder()`].
pub struct Agent {
    agent_loop: AgentLoop,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("workspace", &self.tool_context.workspace)
            .finish()
    }
}

impl Agent {
    /// Create a new [`AgentBuilder`] to configure and build an Agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Send a prompt and wait for the complete response (non-streaming).
    pub async fn send(&self, prompt: &str) -> Result<AgentResult> {
        self.agent_loop.execute(&[], prompt, None).await
    }

    /// Send a prompt and receive a stream of events (streaming).
    ///
    /// Returns a channel receiver that yields [`AgentEvent`]s as they occur,
    /// and a join handle for the background execution task.
    pub async fn stream(
        &self,
        prompt: &str,
    ) -> Result<(
        mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<AgentResult>>,
    )> {
        self.agent_loop.execute_streaming(&[], prompt).await
    }

    /// Send a prompt with existing conversation history (non-streaming).
    pub async fn send_with_history(
        &self,
        history: &[crate::llm::Message],
        prompt: &str,
    ) -> Result<AgentResult> {
        self.agent_loop.execute(history, prompt, None).await
    }

    /// Send a prompt with existing conversation history (streaming).
    pub async fn stream_with_history(
        &self,
        history: &[crate::llm::Message],
        prompt: &str,
    ) -> Result<(
        mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<AgentResult>>,
    )> {
        self.agent_loop.execute_streaming(history, prompt).await
    }

    // ========================================================================
    // Direct tool execution (bypasses LLM)
    // ========================================================================

    /// Execute a tool directly by name, bypassing the LLM.
    pub async fn tool(&self, name: &str, args: serde_json::Value) -> Result<ToolResult> {
        self.tool_executor
            .execute_with_context(name, &args, &self.tool_context)
            .await
    }

    /// Read a file directly (convenience wrapper around the `read` tool).
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let args = serde_json::json!({ "path": path });
        let result = self.tool("read", args).await?;
        if result.exit_code != 0 {
            anyhow::bail!("read_file failed: {}", result.output);
        }
        Ok(result.output)
    }

    /// Execute a bash command directly (convenience wrapper around the `bash` tool).
    pub async fn bash(&self, command: &str) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        let result = self.tool("bash", args).await?;
        if result.exit_code != 0 {
            anyhow::bail!(
                "bash command failed (exit {}): {}",
                result.exit_code,
                result.output
            );
        }
        Ok(result.output)
    }

    /// Search files with a glob pattern (convenience wrapper around the `glob` tool).
    pub async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool("glob", args).await?;
        if result.exit_code != 0 {
            anyhow::bail!("glob failed: {}", result.output);
        }
        Ok(result.output.lines().map(|l| l.to_string()).collect())
    }

    /// Search file contents with a regex pattern (convenience wrapper around the `grep` tool).
    pub async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool("grep", args).await?;
        if result.exit_code != 0 {
            anyhow::bail!("grep failed: {}", result.output);
        }
        Ok(result.output)
    }
}

// ============================================================================
// AgentBuilder
// ============================================================================

/// Builder for constructing an [`Agent`] with the desired configuration.
pub struct AgentBuilder {
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    workspace: Option<PathBuf>,
    system_prompt: Option<String>,
    thinking_budget: Option<usize>,
    max_tool_rounds: Option<usize>,
    hook_engine: Option<Arc<HookEngine>>,
    extra_tools: Vec<ToolDefinition>,
}

impl AgentBuilder {
    fn new() -> Self {
        Self {
            model: None,
            api_key: None,
            base_url: None,
            workspace: None,
            system_prompt: None,
            thinking_budget: None,
            max_tool_rounds: None,
            hook_engine: None,
            extra_tools: Vec::new(),
        }
    }

    /// Set the LLM model identifier (e.g., "claude-sonnet-4-20250514", "gpt-4o").
    pub fn model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    /// Set the API key for the LLM provider.
    pub fn api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }

    /// Set the base URL for the LLM API (overrides default provider URL).
    pub fn base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.to_string());
        self
    }

    /// Set the workspace directory (sandbox root for tool execution).
    pub fn workspace(mut self, path: impl AsRef<Path>) -> Self {
        self.workspace = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the system prompt for the agent.
    pub fn system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = Some(prompt.to_string());
        self
    }

    /// Set the thinking/reasoning budget in tokens.
    pub fn thinking_budget(mut self, tokens: usize) -> Self {
        self.thinking_budget = Some(tokens);
        self
    }

    /// Set the maximum number of tool execution rounds per turn.
    pub fn max_tool_rounds(mut self, max: usize) -> Self {
        self.max_tool_rounds = Some(max);
        self
    }

    /// Attach a hook engine for lifecycle event interception.
    pub fn with_hooks(mut self, engine: Arc<HookEngine>) -> Self {
        self.hook_engine = Some(engine);
        self
    }

    /// Add extra tool definitions (in addition to the 11 builtins).
    pub fn extra_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.extra_tools = tools;
        self
    }

    /// Build the [`Agent`].
    ///
    /// Internally:
    /// 1. Creates an `LlmClient` (auto-detects Anthropic vs OpenAI from model name)
    /// 2. Creates a `ToolExecutor` with the workspace and builtin tools
    /// 3. Wires everything into an `AgentLoop`
    pub async fn build(self) -> Result<Agent> {
        let model = self.model.context("model is required")?;
        let api_key = self.api_key.context("api_key is required")?;
        let workspace = self
            .workspace
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        // Auto-detect provider from model name
        let llm_client: Arc<dyn LlmClient> =
            if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
                let mut client = OpenAiClient::new(api_key, model);
                if let Some(url) = self.base_url {
                    client = client.with_base_url(url);
                }
                Arc::new(client)
            } else {
                // Default to Anthropic for claude-* and any other model names
                let mut client = AnthropicClient::new(api_key, model);
                if let Some(url) = self.base_url {
                    client = client.with_base_url(url);
                }
                Arc::new(client)
            };

        // Create tool executor
        let tool_executor = Arc::new(ToolExecutor::new(workspace.to_string_lossy().to_string()));
        let tool_context = ToolContext::new(workspace);

        // Build tool definitions from executor + any extras
        let mut tool_defs = tool_executor.definitions();
        tool_defs.extend(self.extra_tools);

        // Build agent config
        let config = AgentConfig {
            system_prompt: self.system_prompt,
            tools: tool_defs,
            max_tool_rounds: self.max_tool_rounds.unwrap_or(AgentConfig::default().max_tool_rounds),
            hook_engine: self.hook_engine,
            ..AgentConfig::default()
        };

        // Create agent loop
        let agent_loop = AgentLoop::new(
            llm_client,
            tool_executor.clone(),
            tool_context.clone(),
            config,
        );

        Ok(Agent {
            agent_loop,
            tool_executor,
            tool_context,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_requires_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(Agent::builder().api_key("test").build());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("model"),
            "Error should mention missing model"
        );
    }

    #[test]
    fn test_builder_requires_api_key() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(Agent::builder().model("claude-sonnet-4-20250514").build());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("api_key"),
            "Error should mention missing api_key"
        );
    }

    #[tokio::test]
    async fn test_builder_creates_anthropic_client() {
        let agent = Agent::builder()
            .model("claude-sonnet-4-20250514")
            .api_key("test-key")
            .workspace("/tmp/test-workspace")
            .build()
            .await;
        assert!(agent.is_ok(), "Should build agent with Anthropic model");
    }

    #[tokio::test]
    async fn test_builder_creates_openai_client() {
        let agent = Agent::builder()
            .model("gpt-4o")
            .api_key("test-key")
            .workspace("/tmp/test-workspace")
            .build()
            .await;
        assert!(agent.is_ok(), "Should build agent with OpenAI model");
    }

    #[tokio::test]
    async fn test_builder_with_all_options() {
        let agent = Agent::builder()
            .model("claude-sonnet-4-20250514")
            .api_key("test-key")
            .workspace("/tmp/test-workspace")
            .system_prompt("You are a helpful assistant.")
            .max_tool_rounds(10)
            .thinking_budget(4096)
            .build()
            .await;
        assert!(agent.is_ok(), "Should build agent with all options set");
    }

    #[tokio::test]
    async fn test_builder_defaults_workspace_to_cwd() {
        let agent = Agent::builder()
            .model("claude-sonnet-4-20250514")
            .api_key("test-key")
            .build()
            .await;
        assert!(
            agent.is_ok(),
            "Should default workspace to current directory"
        );
    }
}
