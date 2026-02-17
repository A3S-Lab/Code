//! Agent Facade API
//!
//! High-level, ergonomic API for using A3S Code as an embedded library.
//!
//! ## Example
//!
//! ```rust,no_run
//! use a3s_code_core::Agent;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let agent = Agent::new("agent.hcl").await?;
//! let session = agent.session("/my-project", None)?;
//! let result = session.send("Explain the auth module").await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::config::CodeConfig;
use crate::error::Result;
use crate::llm::{LlmClient, Message};
use crate::tools::{ToolContext, ToolExecutor};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ============================================================================
// ToolCallResult
// ============================================================================

/// Result of a direct tool execution (no LLM).
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub name: String,
    pub output: String,
    pub exit_code: i32,
}

// ============================================================================
// SessionOptions
// ============================================================================

/// Optional per-session overrides.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Override the default model. Format: `"provider/model"` (e.g., `"openai/gpt-4o"`).
    pub model: Option<String>,
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

// ============================================================================
// Agent
// ============================================================================

/// High-level agent facade.
///
/// Holds the LLM client and agent config. Workspace-independent.
/// Use [`Agent::session()`] to bind to a workspace.
pub struct Agent {
    llm_client: Arc<dyn LlmClient>,
    code_config: CodeConfig,
    config: AgentConfig,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent").finish()
    }
}

impl Agent {
    /// Create from a config file path or inline config string.
    ///
    /// Auto-detects: file path (.hcl/.json) vs inline JSON vs inline HCL.
    pub async fn new(config_source: impl Into<String>) -> Result<Self> {
        let source = config_source.into();
        let path = Path::new(&source);

        let config = if path.extension().is_some() && path.exists() {
            CodeConfig::from_file(path)
                .with_context(|| format!("Failed to load config: {}", path.display()))?
        } else {
            CodeConfig::from_json(&source)
                .or_else(|_| CodeConfig::from_hcl(&source))
                .context("Failed to parse config as JSON or HCL")?
        };

        Self::from_config(config).await
    }

    /// Create from a config file path or inline config string.
    ///
    /// Alias for [`Agent::new()`] — provides a consistent API with
    /// the Python and Node.js SDKs.
    pub async fn create(config_source: impl Into<String>) -> Result<Self> {
        Self::new(config_source).await
    }

    /// Create from a [`CodeConfig`] struct.
    pub async fn from_config(config: CodeConfig) -> Result<Self> {
        let llm_config = config
            .default_llm_config()
            .context("default_model must be set in 'provider/model' format with a valid API key")?;
        let llm_client = crate::llm::create_client_with_config(llm_config);

        let agent_config = AgentConfig {
            max_tool_rounds: config
                .max_tool_rounds
                .unwrap_or(AgentConfig::default().max_tool_rounds),
            ..AgentConfig::default()
        };

        Ok(Agent {
            llm_client,
            code_config: config,
            config: agent_config,
        })
    }

    /// Bind to a workspace directory, returning an [`AgentSession`].
    ///
    /// Pass `None` for defaults, or `Some(SessionOptions)` to override
    /// the model for this session.
    pub fn session(
        &self,
        workspace: impl Into<String>,
        options: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        let opts = options.unwrap_or_default();

        let llm_client = if let Some(ref model) = opts.model {
            let (provider_name, model_id) = model
                .split_once('/')
                .context("model format must be 'provider/model' (e.g., 'openai/gpt-4o')")?;

            let llm_config = self
                .code_config
                .llm_config(provider_name, model_id)
                .with_context(|| {
                    format!("provider '{provider_name}' or model '{model_id}' not found in config")
                })?;

            crate::llm::create_client_with_config(llm_config)
        } else {
            self.llm_client.clone()
        };

        Ok(self.build_session(workspace.into(), llm_client))
    }

    fn build_session(&self, workspace: String, llm_client: Arc<dyn LlmClient>) -> AgentSession {
        let canonical =
            std::fs::canonicalize(&workspace).unwrap_or_else(|_| PathBuf::from(&workspace));

        let tool_executor = Arc::new(ToolExecutor::new(canonical.display().to_string()));
        let tool_defs = tool_executor.definitions();

        let config = AgentConfig {
            tools: tool_defs,
            ..self.config.clone()
        };

        AgentSession {
            llm_client,
            tool_executor,
            tool_context: ToolContext::new(canonical.clone()),
            config,
            workspace: canonical,
            history: Vec::new(),
        }
    }
}

// ============================================================================
// AgentSession
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
pub struct AgentSession {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    config: AgentConfig,
    workspace: PathBuf,
    history: Vec<Message>,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("workspace", &self.workspace.display().to_string())
            .finish()
    }
}

impl AgentSession {
    /// Send a prompt and wait for the complete response.
    pub async fn send(&self, prompt: &str) -> Result<AgentResult> {
        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            self.tool_context.clone(),
            self.config.clone(),
        );
        Ok(agent_loop.execute(&self.history, prompt, None).await?)
    }

    /// Send a prompt with conversation history.
    pub async fn send_with_history(
        &self,
        history: &[Message],
        prompt: &str,
    ) -> Result<AgentResult> {
        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            self.tool_context.clone(),
            self.config.clone(),
        );
        Ok(agent_loop.execute(history, prompt, None).await?)
    }

    /// Send a prompt and stream events back.
    pub async fn stream(
        &self,
        prompt: &str,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        let (tx, rx) = mpsc::channel(256);
        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            self.tool_context.clone(),
            self.config.clone(),
        );
        let history = self.history.clone();
        let prompt = prompt.to_string();

        let handle = tokio::spawn(async move {
            let _ = agent_loop.execute(&history, &prompt, Some(tx)).await;
        });

        Ok((rx, handle))
    }

    /// Read a file from the workspace.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let args = serde_json::json!({ "file_path": path });
        let result = self.tool_executor.execute("read", &args).await?;
        Ok(result.output)
    }

    /// Execute a bash command in the workspace.
    pub async fn bash(&self, command: &str) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        let result = self.tool_executor.execute("bash", &args).await?;
        Ok(result.output)
    }

    /// Search for files matching a glob pattern.
    pub async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("glob", &args).await?;
        let files: Vec<String> = result
            .output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        Ok(files)
    }

    /// Search file contents with a regex pattern.
    pub async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("grep", &args).await?;
        Ok(result.output)
    }

    /// Execute a tool by name, bypassing the LLM.
    pub async fn tool(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        let result = self.tool_executor.execute(name, &args).await?;
        Ok(ToolCallResult {
            name: name.to_string(),
            output: result.output,
            exit_code: result.exit_code,
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

    fn test_config() -> CodeConfig {
        CodeConfig {
            default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
            providers: vec![
                ProviderConfig {
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
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: Some("test-openai-key".to_string()),
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
        }
    }

    #[tokio::test]
    async fn test_from_config() {
        let agent = Agent::from_config(test_config()).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_session_default() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace", None);
        assert!(session.is_ok());
        let debug = format!("{:?}", session.unwrap());
        assert!(debug.contains("AgentSession"));
    }

    #[tokio::test]
    async fn test_session_with_model_override() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("openai/gpt-4o");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_ok());
    }

    #[tokio::test]
    async fn test_session_with_invalid_model_format() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("gpt-4o");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_err());
    }

    #[tokio::test]
    async fn test_session_with_model_not_found() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("openai/nonexistent");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_err());
    }

    #[tokio::test]
    async fn test_new_with_json_string() {
        let json = r#"{
            "defaultModel": "anthropic/claude-sonnet-4-20250514",
            "providers": [{
                "name": "anthropic",
                "apiKey": "test-key",
                "models": [{"id": "claude-sonnet-4-20250514", "name": "Claude Sonnet 4"}]
            }]
        }"#;
        let agent = Agent::new(json).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_new_with_hcl_string() {
        let hcl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers {
                name    = "anthropic"
                api_key = "test-key"
                models {
                    id   = "claude-sonnet-4-20250514"
                    name = "Claude Sonnet 4"
                }
            }
        "#;
        let agent = Agent::new(hcl).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_create_alias_json() {
        let json = r#"{
            "defaultModel": "anthropic/claude-sonnet-4-20250514",
            "providers": [{
                "name": "anthropic",
                "apiKey": "test-key",
                "models": [{"id": "claude-sonnet-4-20250514", "name": "Claude Sonnet 4"}]
            }]
        }"#;
        let agent = Agent::create(json).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_create_alias_hcl() {
        let hcl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers {
                name    = "anthropic"
                api_key = "test-key"
                models {
                    id   = "claude-sonnet-4-20250514"
                    name = "Claude Sonnet 4"
                }
            }
        "#;
        let agent = Agent::create(hcl).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_create_and_new_produce_same_result() {
        let json = r#"{
            "defaultModel": "anthropic/claude-sonnet-4-20250514",
            "providers": [{
                "name": "anthropic",
                "apiKey": "test-key",
                "models": [{"id": "claude-sonnet-4-20250514", "name": "Claude Sonnet 4"}]
            }]
        }"#;
        let agent_new = Agent::new(json).await;
        let agent_create = Agent::create(json).await;
        assert!(agent_new.is_ok());
        assert!(agent_create.is_ok());

        // Both should produce working sessions
        let session_new = agent_new.unwrap().session("/tmp/test-ws-new", None);
        let session_create = agent_create.unwrap().session("/tmp/test-ws-create", None);
        assert!(session_new.is_ok());
        assert!(session_create.is_ok());
    }

    #[test]
    fn test_from_config_requires_default_model() {
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
        let result = rt.block_on(Agent::from_config(config));
        assert!(result.is_err());
    }
}
