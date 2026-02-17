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
//! let result = session.send("Explain the auth module", None).await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::config::CodeConfig;
use crate::error::Result;
use crate::llm::{LlmClient, Message};
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionLane, SessionQueueConfig,
    SessionQueueStats,
};
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::skill::Skill;
use crate::tools::{load_skills, ToolContext, ToolExecutor};
use a3s_lane::{DeadLetter, MetricsSnapshot};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc};
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
    /// Extra directories to scan for skill files (`.md` with YAML frontmatter).
    /// Merged with any global `skill_dirs` from [`CodeConfig`].
    pub skill_dirs: Vec<PathBuf>,
    /// Extra directories to scan for agent files.
    /// Merged with any global `agent_dirs` from [`CodeConfig`].
    pub agent_dirs: Vec<PathBuf>,
    /// Optional queue configuration for lane-based tool execution.
    ///
    /// When set, enables priority-based tool scheduling with parallel execution
    /// of read-only (Query-lane) tools, DLQ, metrics, and external task handling.
    pub queue_config: Option<SessionQueueConfig>,
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_skill_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.skill_dirs.push(dir.into());
        self
    }

    pub fn with_agent_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.agent_dirs.push(dir.into());
        self
    }

    pub fn with_queue_config(mut self, config: SessionQueueConfig) -> Self {
        self.queue_config = Some(config);
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
    /// the model, skill directories, or agent directories for this session.
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

        self.build_session(workspace.into(), llm_client, &opts)
    }

    fn build_session(
        &self,
        workspace: String,
        llm_client: Arc<dyn LlmClient>,
        opts: &SessionOptions,
    ) -> Result<AgentSession> {
        let canonical =
            std::fs::canonicalize(&workspace).unwrap_or_else(|_| PathBuf::from(&workspace));

        let tool_executor = Arc::new(ToolExecutor::new(canonical.display().to_string()));
        let tool_defs = tool_executor.definitions();

        // Load skills from per-session dirs first, then global dirs
        let mut skill_filters: Vec<Skill> = Vec::new();
        for dir in &opts.skill_dirs {
            skill_filters.extend(load_skills(dir));
        }
        for dir in &self.code_config.skill_dirs {
            skill_filters.extend(load_skills(dir));
        }

        let config = AgentConfig {
            tools: tool_defs,
            skill_tool_filters: skill_filters,
            ..self.config.clone()
        };

        // Create lane queue if configured
        let command_queue = if let Some(ref queue_config) = opts.queue_config {
            let (event_tx, _) = broadcast::channel(256);
            let session_id = uuid::Uuid::new_v4().to_string();
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    // We're inside an async runtime — use block_in_place
                    let queue = tokio::task::block_in_place(|| {
                        handle.block_on(SessionLaneQueue::new(
                            &session_id,
                            queue_config.clone(),
                            event_tx,
                        ))
                    });
                    match queue {
                        Ok(q) => {
                            // Start the queue
                            let q = Arc::new(q);
                            let q2 = Arc::clone(&q);
                            tokio::task::block_in_place(|| {
                                handle.block_on(async { q2.start().await.ok() })
                            });
                            Some(q)
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create session lane queue: {}", e);
                            None
                        }
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        "No async runtime available for queue creation — queue disabled"
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(AgentSession {
            llm_client,
            tool_executor,
            tool_context: ToolContext::new(canonical.clone()),
            config,
            workspace: canonical,
            history: RwLock::new(Vec::new()),
            command_queue,
        })
    }
}

// ============================================================================
// AgentSession
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
///
/// History is automatically accumulated after each `send()` call.
/// Use `history()` to retrieve the current conversation log.
pub struct AgentSession {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    config: AgentConfig,
    workspace: PathBuf,
    /// Internal conversation history, auto-updated after each `send()`.
    history: RwLock<Vec<Message>>,
    /// Optional lane queue for priority-based tool execution with parallelism.
    command_queue: Option<Arc<SessionLaneQueue>>,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("workspace", &self.workspace.display().to_string())
            .finish()
    }
}

impl AgentSession {
    /// Build an `AgentLoop` with the session's configuration.
    ///
    /// Propagates the lane queue (if configured) to enable parallel
    /// Query-lane tool execution.
    fn build_agent_loop(&self) -> AgentLoop {
        let mut agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            self.tool_context.clone(),
            self.config.clone(),
        );
        if let Some(ref queue) = self.command_queue {
            agent_loop = agent_loop.with_queue(Arc::clone(queue));
        }
        agent_loop
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// When `history` is `None`, uses (and auto-updates) the session's
    /// internal conversation history. When `Some`, uses the provided
    /// history instead (the internal history is **not** modified).
    pub async fn send(
        &self,
        prompt: &str,
        history: Option<&[Message]>,
    ) -> Result<AgentResult> {
        let agent_loop = self.build_agent_loop();

        let use_internal = history.is_none();
        let effective_history = match history {
            Some(h) => h.to_vec(),
            None => self.history.read().unwrap().clone(),
        };

        let result = agent_loop.execute(&effective_history, prompt, None).await?;

        // Auto-accumulate: only update internal history when no custom
        // history was provided.
        if use_internal {
            *self.history.write().unwrap() = result.messages.clone();
        }

        Ok(result)
    }

    /// Send a prompt and stream events back.
    ///
    /// When `history` is `None`, uses the session's internal history
    /// (note: streaming does **not** auto-update internal history since
    /// the result is consumed asynchronously via the channel).
    /// When `Some`, uses the provided history instead.
    pub async fn stream(
        &self,
        prompt: &str,
        history: Option<&[Message]>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        let (tx, rx) = mpsc::channel(256);
        let agent_loop = self.build_agent_loop();
        let effective_history = match history {
            Some(h) => h.to_vec(),
            None => self.history.read().unwrap().clone(),
        };
        let prompt = prompt.to_string();

        let handle = tokio::spawn(async move {
            let _ = agent_loop.execute(&effective_history, &prompt, Some(tx)).await;
        });

        Ok((rx, handle))
    }

    /// Return a snapshot of the session's conversation history.
    pub fn history(&self) -> Vec<Message> {
        self.history.read().unwrap().clone()
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

    // ========================================================================
    // Queue API
    // ========================================================================

    /// Returns whether this session has a lane queue configured.
    pub fn has_queue(&self) -> bool {
        self.command_queue.is_some()
    }

    /// Configure a lane's handler mode (Internal/External/Hybrid).
    ///
    /// Only effective when a queue is configured via `SessionOptions::with_queue_config`.
    pub async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        if let Some(ref queue) = self.command_queue {
            queue.set_lane_handler(lane, config).await;
        }
    }

    /// Complete an external task by ID.
    ///
    /// Returns `true` if the task was found and completed, `false` if not found.
    pub async fn complete_external_task(
        &self,
        task_id: &str,
        result: ExternalTaskResult,
    ) -> bool {
        if let Some(ref queue) = self.command_queue {
            queue.complete_external_task(task_id, result).await
        } else {
            false
        }
    }

    /// Get pending external tasks awaiting completion by an external handler.
    pub async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        if let Some(ref queue) = self.command_queue {
            queue.pending_external_tasks().await
        } else {
            Vec::new()
        }
    }

    /// Get queue statistics (pending, active, external counts per lane).
    pub async fn queue_stats(&self) -> SessionQueueStats {
        if let Some(ref queue) = self.command_queue {
            queue.stats().await
        } else {
            SessionQueueStats::default()
        }
    }

    /// Get a metrics snapshot from the queue (if metrics are enabled).
    pub async fn queue_metrics(&self) -> Option<MetricsSnapshot> {
        if let Some(ref queue) = self.command_queue {
            queue.metrics_snapshot().await
        } else {
            None
        }
    }

    /// Get dead letters from the queue's DLQ (if DLQ is enabled).
    pub async fn dead_letters(&self) -> Vec<DeadLetter> {
        if let Some(ref queue) = self.command_queue {
            queue.dead_letters().await
        } else {
            Vec::new()
        }
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

    #[tokio::test]
    async fn test_history_empty_on_new_session() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace", None).unwrap();
        assert!(session.history().is_empty());
    }

    #[tokio::test]
    async fn test_session_options_with_skill_dir() {
        let opts = SessionOptions::new()
            .with_skill_dir("/tmp/skills")
            .with_skill_dir("/tmp/more-skills");
        assert_eq!(opts.skill_dirs.len(), 2);
        assert_eq!(opts.skill_dirs[0], PathBuf::from("/tmp/skills"));
        assert_eq!(opts.skill_dirs[1], PathBuf::from("/tmp/more-skills"));
    }

    #[tokio::test]
    async fn test_session_options_with_agent_dir() {
        let opts = SessionOptions::new()
            .with_agent_dir("/tmp/agents")
            .with_agent_dir("/tmp/more-agents");
        assert_eq!(opts.agent_dirs.len(), 2);
        assert_eq!(opts.agent_dirs[0], PathBuf::from("/tmp/agents"));
        assert_eq!(opts.agent_dirs[1], PathBuf::from("/tmp/more-agents"));
    }

    #[tokio::test]
    async fn test_session_options_combined() {
        let opts = SessionOptions::new()
            .with_model("openai/gpt-4o")
            .with_skill_dir("/tmp/skills")
            .with_agent_dir("/tmp/agents");
        assert_eq!(opts.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(opts.skill_dirs.len(), 1);
        assert_eq!(opts.agent_dirs.len(), 1);
    }

    #[tokio::test]
    async fn test_session_with_skill_dirs() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        // Non-existent dir is fine — load_skills handles it gracefully
        let opts = SessionOptions::new().with_skill_dir("/tmp/nonexistent-skills");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_ok());
    }

    #[tokio::test]
    async fn test_session_with_real_skill_dir() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skill_dir).unwrap();

        // Write a valid skill file
        std::fs::write(
            skill_dir.join("test-skill.md"),
            "---\nname: test-skill\ndescription: A test skill\nallowed-tools: Bash(*)\n---\nTest instructions.\n",
        ).unwrap();

        let opts = SessionOptions::new().with_skill_dir(&skill_dir);
        let session = agent.session("/tmp/test-workspace", Some(opts)).unwrap();
        // Verify the skill was loaded into the config
        assert_eq!(session.config.skill_tool_filters.len(), 1);
        assert_eq!(session.config.skill_tool_filters[0].name, "test-skill");
    }

    // ========================================================================
    // Queue Integration Tests
    // ========================================================================

    #[test]
    fn test_session_options_with_queue_config() {
        let qc = SessionQueueConfig::default().with_lane_features();
        let opts = SessionOptions::new().with_queue_config(qc.clone());
        assert!(opts.queue_config.is_some());

        let config = opts.queue_config.unwrap();
        assert!(config.enable_dlq);
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.default_timeout_ms, Some(60_000));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_with_queue_config() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-queue", Some(opts));
        assert!(session.is_ok());
        let session = session.unwrap();
        assert!(session.has_queue());
    }

    #[tokio::test]
    async fn test_session_without_queue_config() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace-noqueue", None).unwrap();
        assert!(!session.has_queue());
    }

    #[tokio::test]
    async fn test_session_queue_stats_without_queue() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace-stats", None).unwrap();
        let stats = session.queue_stats().await;
        // Without a queue, stats should have zero values
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_stats_with_queue() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-qstats", Some(opts)).unwrap();
        let stats = session.queue_stats().await;
        // Fresh queue with no commands should have zero stats
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_pending_external_tasks_empty() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-ext", Some(opts)).unwrap();
        let tasks = session.pending_external_tasks().await;
        assert!(tasks.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_dead_letters_empty() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default().with_dlq(Some(100));
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-dlq", Some(opts)).unwrap();
        let dead = session.dead_letters().await;
        assert!(dead.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_metrics_disabled() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        // Metrics not enabled
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-nomet", Some(opts)).unwrap();
        let metrics = session.queue_metrics().await;
        assert!(metrics.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_metrics_enabled() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default().with_metrics();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-met", Some(opts)).unwrap();
        let metrics = session.queue_metrics().await;
        assert!(metrics.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_set_lane_handler() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-handler", Some(opts)).unwrap();

        // Set Execute lane to External mode
        session
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: crate::queue::TaskHandlerMode::External,
                    timeout_ms: 30_000,
                },
            )
            .await;

        // No panic = success. The handler config is stored internally.
        // We can't directly read it back but we verify no errors.
    }
}
