//! Agent Facade API
//!
//! High-level, ergonomic API for using A3S Code as an embedded library.
//! Wraps the internal `AgentLoop`, `ToolExecutor`, and `LlmClient` behind
//! a simple builder pattern.
//!
//! ## Example
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, AgentSession, CodeConfig, ProviderConfig, ModelConfig};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let config = CodeConfig {
//!     default_provider: Some("anthropic".into()),
//!     default_model: Some("claude-sonnet-4-20250514".into()),
//!     providers: vec![ProviderConfig {
//!         name: "anthropic".into(),
//!         api_key: Some("sk-ant-...".into()),
//!         base_url: None,
//!         models: vec![ModelConfig {
//!             id: "claude-sonnet-4-20250514".into(),
//!             name: "Claude Sonnet 4".into(),
//!             ..serde_json::from_value(serde_json::json!({"id":"claude-sonnet-4-20250514"})).unwrap()
//!         }],
//!     }],
//!     ..Default::default()
//! };
//!
//! let agent = Agent::builder()
//!     .with_config(config)
//!     .build()
//!     .await?;
//!
//! // Create a workspace-bound session
//! let session = agent.session("/my-project");
//! let result = session.send("Explain the auth module").await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::config::CodeConfig;
use crate::hooks::HookEngine;
use crate::llm::{LlmClient, ToolDefinition};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

// ============================================================================
// Agent — holds LLM config + tool registry, no workspace
// ============================================================================

/// High-level agent facade for embedded library usage.
///
/// Holds the LLM client and tool executor configuration. Does NOT hold a
/// workspace — workspace is bound per-session via [`Agent::session()`].
///
/// Created via [`Agent::builder()`].
pub struct Agent {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    config: AgentConfig,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent").finish()
    }
}

impl Agent {
    /// Create a new [`AgentBuilder`] to configure and build an Agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Create a workspace-bound session for executing prompts and tools.
    ///
    /// Each session has its own `ToolContext` (workspace root, session ID).
    /// The LLM client and tool executor are shared across all sessions.
    pub fn session(&self, workspace: impl AsRef<Path>) -> AgentSession {
        let workspace = workspace.as_ref().to_path_buf();
        let tool_context = ToolContext::new(workspace);

        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            tool_context.clone(),
            self.config.clone(),
        );

        AgentSession {
            agent_loop,
            tool_executor: self.tool_executor.clone(),
            tool_context,
        }
    }
}

// ============================================================================
// AgentSession — workspace-bound execution context
// ============================================================================

/// A workspace-bound session created from an [`Agent`].
///
/// Provides all prompt execution and direct tool access methods.
/// Each session is tied to a specific workspace directory.
pub struct AgentSession {
    agent_loop: AgentLoop,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("workspace", &self.tool_context.workspace)
            .finish()
    }
}

impl AgentSession {
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
// AgentBuilder — config-only builder
// ============================================================================

/// Builder for constructing an [`Agent`] with the desired configuration.
///
/// All LLM provider/model configuration is supplied via [`CodeConfig`].
/// Use [`with_config()`](AgentBuilder::with_config),
/// [`with_config_file()`](AgentBuilder::with_config_file),
/// [`with_config_json()`](AgentBuilder::with_config_json), or
/// [`with_config_hcl()`](AgentBuilder::with_config_hcl).
pub struct AgentBuilder {
    system_prompt: Option<String>,
    thinking_budget: Option<usize>,
    max_tool_rounds: Option<usize>,
    hook_engine: Option<Arc<HookEngine>>,
    extra_tools: Vec<ToolDefinition>,
    code_config: Option<CodeConfig>,
}

impl AgentBuilder {
    fn new() -> Self {
        Self {
            system_prompt: None,
            thinking_budget: None,
            max_tool_rounds: None,
            hook_engine: None,
            extra_tools: Vec::new(),
            code_config: None,
        }
    }

    /// Set the configuration from a [`CodeConfig`] struct (programmatic JSON).
    pub fn with_config(mut self, config: CodeConfig) -> Self {
        self.code_config = Some(config);
        self
    }

    /// Load configuration from a file path (auto-detects JSON or HCL by extension).
    pub fn with_config_file(mut self, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let config = CodeConfig::from_file(path)?;
        self.code_config = Some(config);
        Ok(self)
    }

    /// Parse configuration from a JSON string.
    pub fn with_config_json(mut self, json: &str) -> Result<Self> {
        let config = CodeConfig::from_json(json)?;
        self.code_config = Some(config);
        Ok(self)
    }

    /// Parse configuration from an HCL string.
    pub fn with_config_hcl(mut self, hcl: &str) -> Result<Self> {
        let config = CodeConfig::from_hcl(hcl)?;
        self.code_config = Some(config);
        Ok(self)
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

    /// Add extra tool definitions (in addition to the builtins).
    pub fn extra_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.extra_tools = tools;
        self
    }

    /// Build the [`Agent`].
    ///
    /// Requires a [`CodeConfig`] with `default_provider` and `default_model` set.
    ///
    /// Internally:
    /// 1. Reads the default LLM config from `CodeConfig`
    /// 2. Creates an `LlmClient` via `create_client_with_config()`
    /// 3. Creates a `ToolExecutor` with a temporary workspace (sessions provide the real one)
    /// 4. Wires everything into an `AgentConfig`
    pub async fn build(self) -> Result<Agent> {
        let code_config = self.code_config.context(
            "config is required — use with_config(), with_config_file(), with_config_json(), or with_config_hcl()",
        )?;

        // Get default LLM config from CodeConfig
        let llm_config = code_config.default_llm_config().context(
            "default_provider and default_model must be set in config with a valid API key",
        )?;

        // Create LLM client
        let llm_client = crate::llm::create_client_with_config(llm_config);

        // Create a shared tool executor (workspace-independent for tool registration).
        // The actual workspace is set per-session via Agent::session().
        let tool_executor = Arc::new(ToolExecutor::new(".".to_string()));

        // Build tool definitions from executor + any extras
        let mut tool_defs = tool_executor.definitions();
        tool_defs.extend(self.extra_tools);

        // Build agent config
        let config = AgentConfig {
            system_prompt: self.system_prompt,
            tools: tool_defs,
            max_tool_rounds: self
                .max_tool_rounds
                .unwrap_or(AgentConfig::default().max_tool_rounds),
            hook_engine: self.hook_engine,
            ..AgentConfig::default()
        };

        Ok(Agent {
            llm_client,
            tool_executor,
            config,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, ModelModalities, ProviderConfig};
    use std::path::PathBuf;

    fn test_config() -> CodeConfig {
        CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4-20250514".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                models: vec![ModelConfig {
                    id: "claude-sonnet-4-20250514".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude-sonnet".to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: Default::default(),
                    limit: Default::default(),
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_builder_requires_config() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(Agent::builder().build());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("config"),
            "Error should mention missing config"
        );
    }

    #[tokio::test]
    async fn test_builder_with_config() {
        let agent = Agent::builder().with_config(test_config()).build().await;
        assert!(agent.is_ok(), "Should build agent with config");
    }

    #[tokio::test]
    async fn test_session_creation() {
        let agent = Agent::builder()
            .with_config(test_config())
            .build()
            .await
            .unwrap();

        let session = agent.session("/tmp/test-workspace");
        assert_eq!(
            format!("{:?}", session),
            format!(
                "AgentSession {{ workspace: \"{}\" }}",
                PathBuf::from("/tmp/test-workspace")
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from("/tmp/test-workspace"))
                    .display()
            )
        );
    }

    #[tokio::test]
    async fn test_builder_with_all_options() {
        let agent = Agent::builder()
            .with_config(test_config())
            .system_prompt("You are a helpful assistant.")
            .max_tool_rounds(10)
            .thinking_budget(4096)
            .build()
            .await;
        assert!(agent.is_ok(), "Should build agent with all options set");
    }

    #[tokio::test]
    async fn test_builder_with_multi_provider_config() {
        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4-20250514".to_string()),
            providers: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: Some("test-key-1".to_string()),
                    base_url: None,
                    models: vec![ModelConfig {
                        id: "claude-sonnet-4-20250514".to_string(),
                        name: "Claude Sonnet 4".to_string(),
                        family: "claude-sonnet".to_string(),
                        api_key: None,
                        base_url: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: Default::default(),
                        limit: Default::default(),
                    }],
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: Some("test-key-2".to_string()),
                    base_url: None,
                    models: vec![ModelConfig {
                        id: "gpt-4o".to_string(),
                        name: "GPT-4o".to_string(),
                        family: "gpt-4".to_string(),
                        api_key: None,
                        base_url: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: Default::default(),
                        limit: Default::default(),
                    }],
                },
            ],
            ..Default::default()
        };

        let agent = Agent::builder().with_config(config).build().await;
        assert!(
            agent.is_ok(),
            "Should build agent with multi-provider config"
        );
    }

    #[tokio::test]
    async fn test_builder_with_config_from_json() {
        let json = r#"{
            "defaultProvider": "anthropic",
            "defaultModel": "claude-sonnet-4-20250514",
            "providers": [{
                "name": "anthropic",
                "apiKey": "test-key",
                "models": [{
                    "id": "claude-sonnet-4-20250514",
                    "name": "Claude Sonnet 4"
                }]
            }]
        }"#;

        let agent = Agent::builder()
            .with_config_json(json)
            .unwrap()
            .build()
            .await;
        assert!(agent.is_ok(), "Should build agent from JSON config string");
    }

    #[tokio::test]
    async fn test_builder_with_config_from_hcl() {
        let hcl = r#"
            default_provider = "anthropic"
            default_model    = "claude-sonnet-4-20250514"

            providers {
                name    = "anthropic"
                api_key = "test-key"

                models {
                    id   = "claude-sonnet-4-20250514"
                    name = "Claude Sonnet 4"
                }
            }
        "#;

        let agent = Agent::builder().with_config_hcl(hcl).unwrap().build().await;
        assert!(agent.is_ok(), "Should build agent from HCL config string");
    }

    #[test]
    fn test_builder_requires_default_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = CodeConfig {
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                models: vec![],
            }],
            ..Default::default()
        };
        let result = rt.block_on(Agent::builder().with_config(config).build());
        assert!(
            result.is_err(),
            "Should fail without default_provider/default_model"
        );
    }
}
