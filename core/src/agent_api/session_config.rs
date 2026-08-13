use super::{Agent, SessionOptions};
use crate::config::CodeConfig;
use crate::error::{CodeError, Result, SessionBuildResource};
use crate::llm::LlmClient;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Fully merged session configuration used by the construction kernel.
///
/// `SessionOptions` remains the public patch type. This internal value is the
/// single source of truth after `CodeConfig + SessionOptions` resolution and
/// owns every resource that required asynchronous initialization.
pub(super) struct ResolvedSessionConfig {
    pub(super) options: SessionOptions,
    pub(super) session_id: String,
    pub(super) llm_client: Arc<dyn LlmClient>,
    pub(super) model_name: String,
    pub(super) queue_config: Option<crate::queue::SessionQueueConfig>,
    pub(super) memory: Arc<crate::memory::AgentMemory>,
    pub(super) session_store: Option<Arc<dyn crate::store::SessionStore>>,
    /// Manager owned exclusively by the built session. Live add/remove calls
    /// never mutate an inherited agent- or host-owned manager.
    pub(super) mcp_manager: Arc<crate::mcp::manager::McpManager>,
    pub(super) inherited_mcp_managers: Vec<Arc<crate::mcp::manager::McpManager>>,
    pub(super) mcp_sources: Vec<ResolvedMcpSource>,
    pub(super) rl_trajectory_recorder: crate::rl_trajectory::RlTrajectoryRecorder,
    pub(super) limits: ResolvedSessionLimits,
}

#[derive(Clone)]
pub(super) struct ResolvedMcpSource {
    pub(super) manager: Arc<crate::mcp::manager::McpManager>,
    pub(super) tools: Vec<(String, crate::mcp::McpTool)>,
}

pub(super) struct ResolvedSessionLimits {
    pub(super) max_parse_retries: u32,
    pub(super) tool_timeout_ms: Option<u64>,
    pub(super) llm_api_timeout_ms: Option<u64>,
    pub(super) circuit_breaker_threshold: u32,
    pub(super) duplicate_tool_call_threshold: u32,
    pub(super) max_tool_rounds: usize,
    pub(super) max_parallel_tasks: usize,
    pub(super) max_execution_time_ms: Option<u64>,
    pub(super) max_context_tokens: usize,
    pub(super) retention: Option<crate::retention::SessionRetentionLimits>,
}

impl ResolvedSessionConfig {
    pub(super) async fn resolve(
        agent: &Agent,
        workspace: &Path,
        mut options: SessionOptions,
    ) -> Result<Self> {
        let session_id = validate_session_options(&options)?;
        let llm_client =
            resolve_session_llm_client(&agent.code_config, &options, Some(&session_id))?;
        let model_name = resolved_model_name(&agent.code_config, &options);
        let queue_config = options
            .queue_config
            .clone()
            .or_else(|| agent.code_config.queue.clone());
        let rl_trajectory_config = resolve_rl_trajectory_config(&options)?;
        let memory = resolve_session_memory(&agent.code_config, &options, workspace).await?;
        let session_store = resolve_session_store(&agent.code_config, &options).await?;
        let rl_trajectory_recorder = resolve_rl_trajectory_recorder(rl_trajectory_config).await?;
        let (mcp_manager, inherited_mcp_managers, mcp_sources) =
            resolve_session_mcp(agent, &options).await?;
        options.queue_config = queue_config.clone();
        options.mcp_manager = Some(Arc::clone(&mcp_manager));
        let limits = resolve_limits(agent, &options, &model_name);

        Ok(Self {
            options,
            session_id,
            llm_client,
            model_name,
            queue_config,
            memory,
            session_store,
            mcp_manager,
            inherited_mcp_managers,
            mcp_sources,
            rl_trajectory_recorder,
            limits,
        })
    }

    pub(super) fn resolve_sync(
        agent: &Agent,
        _workspace: &Path,
        mut options: SessionOptions,
    ) -> Result<Self> {
        let session_id = validate_session_options(&options)?;
        if resolve_rl_trajectory_config(&options)?.is_some() {
            return Err(CodeError::AsyncSessionBuildRequired {
                resource: SessionBuildResource::RlTrajectory,
            });
        }
        if options.workspace_retrieval.is_some() {
            return Err(CodeError::AsyncSessionBuildRequired {
                resource: SessionBuildResource::WorkspaceRetrieval,
            });
        }
        let memory_store =
            options
                .memory_store
                .clone()
                .ok_or(CodeError::AsyncSessionBuildRequired {
                    resource: SessionBuildResource::MemoryStore,
                })?;
        if options.session_store.is_none()
            && (options.file_session_store_dir.is_some()
                || agent.code_config.sessions_dir.is_some())
        {
            return Err(CodeError::AsyncSessionBuildRequired {
                resource: SessionBuildResource::SessionStore,
            });
        }
        let queue_config = options
            .queue_config
            .clone()
            .or_else(|| agent.code_config.queue.clone());
        if queue_config.is_some() {
            return Err(CodeError::AsyncSessionBuildRequired {
                resource: SessionBuildResource::Queue,
            });
        }
        if options.mcp_manager.is_some() {
            return Err(CodeError::AsyncSessionBuildRequired {
                resource: SessionBuildResource::Mcp,
            });
        }

        let llm_client =
            resolve_session_llm_client(&agent.code_config, &options, Some(&session_id))?;
        let model_name = resolved_model_name(&agent.code_config, &options);
        let memory = Arc::new(crate::memory::AgentMemory::with_config_and_observers(
            memory_store,
            agent.code_config.memory.clone().unwrap_or_default(),
            options.memory_observers.clone(),
        ));
        let mcp_manager = Arc::new(crate::mcp::manager::McpManager::new());
        let mut inherited_mcp_managers = Vec::new();
        let mut mcp_sources = Vec::new();
        if let Some(global) = &agent.global_mcp {
            inherited_mcp_managers.push(Arc::clone(global));
            mcp_sources.push(ResolvedMcpSource {
                manager: Arc::clone(global),
                tools: agent
                    .global_mcp_tools
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .clone(),
            });
        }
        mcp_sources.push(ResolvedMcpSource {
            manager: Arc::clone(&mcp_manager),
            tools: Vec::new(),
        });
        options.queue_config = None;
        options.mcp_manager = Some(Arc::clone(&mcp_manager));
        let limits = resolve_limits(agent, &options, &model_name);

        Ok(Self {
            session_store: options.session_store.clone(),
            options,
            session_id,
            llm_client,
            model_name,
            queue_config: None,
            memory,
            mcp_manager,
            inherited_mcp_managers,
            mcp_sources,
            rl_trajectory_recorder: crate::rl_trajectory::RlTrajectoryRecorder::disabled(),
            limits,
        })
    }
}

fn resolve_rl_trajectory_config(
    options: &SessionOptions,
) -> Result<Option<crate::rl_trajectory::RlTrajectoryConfig>> {
    let config = match &options.rl_trajectory {
        Some(config) => Some(config.clone()),
        None => crate::rl_trajectory::RlTrajectoryConfig::from_env().map_err(|error| {
            CodeError::SessionConfiguration {
                field: "rl_trajectory",
                message: format!("{error:#}"),
            }
        })?,
    };
    Ok(config.filter(|config| config.mode != crate::rl_trajectory::RlTrajectoryMode::Off))
}

async fn resolve_rl_trajectory_recorder(
    config: Option<crate::rl_trajectory::RlTrajectoryConfig>,
) -> Result<crate::rl_trajectory::RlTrajectoryRecorder> {
    let Some(config) = config else {
        return Ok(crate::rl_trajectory::RlTrajectoryRecorder::disabled());
    };
    tokio::task::spawn_blocking(move || {
        crate::rl_trajectory::RlTrajectoryRecorder::from_config(Some(config))
    })
    .await
    .map_err(|error| CodeError::SessionInitialization {
        resource: SessionBuildResource::RlTrajectory,
        message: format!("initialization task failed: {error}"),
    })?
    .map_err(|error| CodeError::SessionInitialization {
        resource: SessionBuildResource::RlTrajectory,
        message: format!("{error:#}"),
    })
}

fn validate_session_options(options: &SessionOptions) -> Result<String> {
    if let Some(policy) = &options.tool_result_transform_policy {
        policy
            .validate()
            .map_err(|error| CodeError::SessionConfiguration {
                field: "tool_result_transform_policy",
                message: error.to_string(),
            })?;
    }
    let session_id = options
        .session_id
        .clone()
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "session_id",
            message: "a session id must be assigned before resolution".to_string(),
        })?;
    if session_id.trim().is_empty() {
        return Err(CodeError::SessionConfiguration {
            field: "session_id",
            message: "must not be empty or whitespace".to_string(),
        });
    }
    if options.memory_store.is_some() && options.file_memory_dir.is_some() {
        return Err(CodeError::SessionConfiguration {
            field: "memory_store",
            message: "memory_store and file_memory_dir are mutually exclusive".to_string(),
        });
    }
    if options.session_store.is_some() && options.file_session_store_dir.is_some() {
        return Err(CodeError::SessionConfiguration {
            field: "session_store",
            message: "session_store and file_session_store_dir are mutually exclusive".to_string(),
        });
    }
    if options
        .context_providers
        .iter()
        .any(|provider| provider.cognitive_package_binding().is_some())
    {
        return Err(CodeError::SessionConfiguration {
            field: "cognitive_context",
            message: "typed cognitive providers must be installed with with_cognitive_context so their exact binding is persisted".to_string(),
        });
    }
    if let Some(context) = &options.cognitive_context {
        context
            .binding()
            .validate()
            .map_err(|error| CodeError::SessionConfiguration {
                field: "cognitive_context",
                message: error.to_string(),
            })?;
        if !options.context_providers.is_empty() {
            return Err(CodeError::SessionConfiguration {
                field: "cognitive_context",
                message: "general-purpose context providers cannot accompany an exact cognitive package; workspace instructions and skills remain available through Code-owned providers".to_string(),
            });
        }
    }
    if options
        .auto_compact_threshold
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(invalid_numeric_option(
            "auto_compact_threshold",
            "must be finite and between 0.0 and 1.0 inclusive",
        ));
    }
    if options.max_context_tokens == Some(0) {
        return Err(invalid_numeric_option(
            "max_context_tokens",
            "must be greater than zero",
        ));
    }
    if options
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(invalid_numeric_option(
            "temperature",
            "must be finite and between 0.0 and 1.0 inclusive",
        ));
    }
    if options.tool_timeout_ms == Some(0) {
        return Err(invalid_numeric_option(
            "tool_timeout_ms",
            "must be greater than zero",
        ));
    }
    if options.llm_api_timeout_ms == Some(0) {
        return Err(invalid_numeric_option(
            "llm_api_timeout_ms",
            "must be greater than zero",
        ));
    }
    if options.max_execution_time_ms == Some(0) {
        return Err(invalid_numeric_option(
            "max_execution_time_ms",
            "must be greater than zero",
        ));
    }
    if options.circuit_breaker_threshold == Some(0) {
        return Err(invalid_numeric_option(
            "circuit_breaker_threshold",
            "must be greater than zero",
        ));
    }
    if options.duplicate_tool_call_threshold == Some(0) {
        return Err(invalid_numeric_option(
            "duplicate_tool_call_threshold",
            "must be greater than zero",
        ));
    }
    if options.max_tool_rounds == Some(0) {
        return Err(invalid_numeric_option(
            "max_tool_rounds",
            "must be greater than zero",
        ));
    }
    if options.max_parallel_tasks == Some(0) {
        return Err(invalid_numeric_option(
            "max_parallel_tasks",
            "must be greater than zero",
        ));
    }
    if options.llm_top_logprobs.is_some_and(|value| value > 20) {
        return Err(invalid_numeric_option(
            "llm_top_logprobs",
            "must be at most 20",
        ));
    }
    Ok(session_id)
}

fn invalid_numeric_option(field: &'static str, message: &'static str) -> CodeError {
    CodeError::SessionConfiguration {
        field,
        message: message.to_string(),
    }
}

fn resolved_model_name(code_config: &CodeConfig, options: &SessionOptions) -> String {
    options
        .model
        .clone()
        .or_else(|| code_config.default_model.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_limits(
    agent: &Agent,
    options: &SessionOptions,
    model_name: &str,
) -> ResolvedSessionLimits {
    let base = &agent.config;
    ResolvedSessionLimits {
        max_parse_retries: options.max_parse_retries.unwrap_or(base.max_parse_retries),
        tool_timeout_ms: options.tool_timeout_ms.or(base.tool_timeout_ms),
        llm_api_timeout_ms: options
            .llm_api_timeout_ms
            .or(base.llm_api_timeout_ms)
            .or(agent.code_config.llm_api_timeout_ms),
        circuit_breaker_threshold: options
            .circuit_breaker_threshold
            .unwrap_or(base.circuit_breaker_threshold),
        duplicate_tool_call_threshold: options
            .duplicate_tool_call_threshold
            .unwrap_or(base.duplicate_tool_call_threshold),
        max_tool_rounds: options.max_tool_rounds.unwrap_or(base.max_tool_rounds),
        max_parallel_tasks: options
            .max_parallel_tasks
            .unwrap_or(base.max_parallel_tasks)
            .max(1),
        max_execution_time_ms: options.max_execution_time_ms.or(base.max_execution_time_ms),
        max_context_tokens: options
            .max_context_tokens
            .or_else(|| configured_model_context_tokens(&agent.code_config, model_name))
            .unwrap_or(base.max_context_tokens),
        retention: Some(options.retention_limits.unwrap_or_default()),
    }
}

fn configured_model_context_tokens(code_config: &CodeConfig, model_name: &str) -> Option<usize> {
    let (provider_name, model_id) = model_name.split_once('/')?;
    let context = code_config
        .find_provider(provider_name)?
        .find_model(model_id)?
        .limit
        .context;
    (context > 0).then_some(context as usize)
}

pub(super) fn resolve_auto_delegation_config(
    code_config: &CodeConfig,
    opts: &SessionOptions,
) -> crate::config::AutoDelegationConfig {
    let mut auto_delegation = if let Some(config) = opts.auto_delegation.clone() {
        config
    } else {
        let mut config = code_config.auto_delegation.clone();
        if let Some(auto_parallel) = code_config.auto_parallel {
            config.auto_parallel = auto_parallel;
        }
        config
    };
    if let Some(enabled) = opts.manual_delegation_enabled {
        auto_delegation.allow_manual_delegation = enabled;
    }
    if let Some(auto_parallel) = opts.auto_parallel_delegation {
        auto_delegation.auto_parallel = auto_parallel;
    }

    auto_delegation
}

pub(super) fn resolve_session_llm_client(
    code_config: &CodeConfig,
    opts: &SessionOptions,
    session_id: Option<&str>,
) -> Result<Arc<dyn LlmClient>> {
    // A host-supplied client overrides the provider-string factory entirely:
    // the host owns the full Action-layer dependency (custom provider, replay
    // client, proxy/audit wrapper). Config-based model resolution is bypassed.
    if let Some(ref client) = opts.llm_client {
        return Ok(Arc::clone(client));
    }

    let model_ref = if let Some(ref model) = opts.model {
        model.as_str()
    } else {
        if opts.temperature.is_some() || opts.thinking_budget.is_some() {
            tracing::warn!(
                "temperature/thinking_budget set without model override - these will be ignored. \
                 Use with_model() to apply LLM parameter overrides."
            );
        }
        code_config
            .default_model
            .as_deref()
            .ok_or_else(|| CodeError::SessionConfiguration {
                field: "model",
                message: "default_model must be set in 'provider/model' format".to_string(),
            })?
    };

    let (provider_name, model_id) =
        model_ref
            .split_once('/')
            .ok_or_else(|| CodeError::SessionConfiguration {
                field: "model",
                message: "must use 'provider/model' format (for example 'openai/gpt-4o')"
                    .to_string(),
            })?;
    if provider_name.trim().is_empty() || model_id.trim().is_empty() {
        return Err(CodeError::SessionConfiguration {
            field: "model",
            message: "provider and model names must both be non-empty".to_string(),
        });
    }

    let mut llm_config = code_config
        .llm_config(provider_name, model_id)
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "model",
            message: format!(
                "provider '{provider_name}' or model '{model_id}' was not found, or has no API key"
            ),
        })?;

    if opts.model.is_some() {
        if let Some(temp) = opts.temperature {
            llm_config = llm_config.with_temperature(temp);
        }
        if let Some(budget) = opts.thinking_budget {
            llm_config = llm_config.with_thinking_budget(budget);
        }
    }

    if let Some(timeout_ms) = opts.llm_api_timeout_ms {
        llm_config = llm_config.with_api_timeout(timeout_ms);
    }

    let logprobs = opts
        .llm_logprobs
        .or_else(|| env_bool("A3S_CODE_LLM_LOGPROBS"))
        .or_else(|| env_bool("A3S_CODE_OPENAI_LOGPROBS"));
    if let Some(enabled) = logprobs {
        llm_config = llm_config.with_logprobs(enabled);
    }

    let top_logprobs = opts
        .llm_top_logprobs
        .or_else(|| env_usize("A3S_CODE_LLM_TOP_LOGPROBS"))
        .or_else(|| env_usize("A3S_CODE_OPENAI_TOP_LOGPROBS"));
    if let Some(top_logprobs) = top_logprobs {
        llm_config = llm_config.with_top_logprobs(top_logprobs);
    }

    if let Some(session_id) = session_id {
        llm_config = llm_config.with_session_id(session_id);
    }

    Ok(crate::llm::create_client_with_config(llm_config))
}

fn env_bool(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
}

async fn resolve_session_memory(
    code_config: &CodeConfig,
    opts: &SessionOptions,
    workspace: &Path,
) -> Result<Arc<crate::memory::AgentMemory>> {
    let store = if let Some(ref store) = opts.memory_store {
        Arc::clone(store)
    } else {
        let dir = opts
            .file_memory_dir
            .clone()
            .or_else(|| code_config.memory_dir.clone())
            .unwrap_or_else(|| default_memory_dir(workspace));
        let store = a3s_memory::FileMemoryStore::new(&dir)
            .await
            .map_err(|error| CodeError::SessionInitialization {
                resource: SessionBuildResource::MemoryStore,
                message: format!("{}: {error:#}", dir.display()),
            })?;
        Arc::new(store) as Arc<dyn a3s_memory::MemoryStore>
    };

    let memory_config = code_config.memory.clone().unwrap_or_default();
    Ok(Arc::new(
        crate::memory::AgentMemory::with_config_and_observers(
            store,
            memory_config,
            opts.memory_observers.clone(),
        ),
    ))
}

fn default_memory_dir(workspace: &Path) -> PathBuf {
    workspace.join(".a3s").join("memory")
}

pub(super) async fn resolve_session_store(
    code_config: &CodeConfig,
    opts: &SessionOptions,
) -> Result<Option<Arc<dyn crate::store::SessionStore>>> {
    if let Some(store) = &opts.session_store {
        return Ok(Some(Arc::clone(store)));
    }

    let Some(dir) = opts
        .file_session_store_dir
        .as_ref()
        .or(code_config.sessions_dir.as_ref())
    else {
        return Ok(None);
    };
    let store = crate::store::FileSessionStore::new(dir)
        .await
        .map_err(|error| CodeError::SessionInitialization {
            resource: SessionBuildResource::SessionStore,
            message: format!("{}: {error:#}", dir.display()),
        })?;
    Ok(Some(Arc::new(store) as Arc<dyn crate::store::SessionStore>))
}

async fn resolve_session_mcp(
    agent: &Agent,
    options: &SessionOptions,
) -> Result<(
    Arc<crate::mcp::manager::McpManager>,
    Vec<Arc<crate::mcp::manager::McpManager>>,
    Vec<ResolvedMcpSource>,
)> {
    // This manager is always private to the new session. `SessionOptions::with_mcp`
    // contributes an inherited capability source; it is never used as the target
    // of live add/remove operations.
    let session = Arc::new(crate::mcp::manager::McpManager::new());
    let mut sources = Vec::new();
    let mut inherited = Vec::new();
    if let Some(global) = &agent.global_mcp {
        inherited.push(Arc::clone(global));
        sources.push(ResolvedMcpSource {
            manager: Arc::clone(global),
            tools: agent
                .global_mcp_tools
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
        });
    }
    if let Some(configured) = &options.mcp_manager {
        let duplicate_global = agent
            .global_mcp
            .as_ref()
            .is_some_and(|global| Arc::ptr_eq(global, configured));
        if !duplicate_global {
            inherited.push(Arc::clone(configured));
            sources.push(ResolvedMcpSource {
                manager: Arc::clone(configured),
                tools: configured.get_all_tools().await,
            });
        }
    }
    sources.push(ResolvedMcpSource {
        manager: Arc::clone(&session),
        tools: Vec::new(),
    });

    Ok((session, inherited, sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmResponse, Message, StreamEvent, ToolDefinition};
    // The LlmClient trait returns anyhow::Result; shadow super's crate::error::Result.
    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    struct DummyClient;

    #[async_trait]
    impl LlmClient for DummyClient {
        async fn complete(
            &self,
            _: &[Message],
            _: Option<&str>,
            _: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            anyhow::bail!("resolver short-circuits before the client is called")
        }

        async fn complete_streaming(
            &self,
            _: &[Message],
            _: Option<&str>,
            _: &[ToolDefinition],
            _: CancellationToken,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("not used")
        }
    }

    // A default CodeConfig has no default_model, so the provider-string factory
    // path errors — proving the override is what makes resolution succeed.
    #[test]
    fn host_supplied_llm_client_overrides_factory() {
        let config = CodeConfig::default();
        let opts = SessionOptions::new().with_llm_client(Arc::new(DummyClient));
        assert!(
            resolve_session_llm_client(&config, &opts, None).is_ok(),
            "with_llm_client must bypass provider/model config resolution"
        );
    }

    #[test]
    fn without_llm_client_missing_default_model_errors() {
        let config = CodeConfig::default();
        let opts = SessionOptions::new();
        assert!(
            resolve_session_llm_client(&config, &opts, None).is_err(),
            "no host client + no default_model should error (control case)"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn session_memory_resolver_always_returns_memory() {
        let workspace = tempfile::tempdir().unwrap();
        let resolved = resolve_session_memory(
            &CodeConfig::default(),
            &SessionOptions::new(),
            workspace.path(),
        )
        .await
        .unwrap();

        resolved
            .remember(a3s_memory::MemoryItem::new("default memory is mandatory"))
            .await
            .unwrap();

        assert_eq!(
            resolved.stats().await.unwrap().long_term_count,
            1,
            "default session memory should be immediately usable"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_memory_resolver_works_on_current_thread_runtime() {
        let workspace = tempfile::tempdir().unwrap();
        let resolved = resolve_session_memory(
            &CodeConfig::default(),
            &SessionOptions::new(),
            workspace.path(),
        )
        .await
        .unwrap();

        resolved
            .remember(a3s_memory::MemoryItem::new(
                "default memory works on current-thread runtimes",
            ))
            .await
            .unwrap();

        assert_eq!(resolved.stats().await.unwrap().long_term_count, 1);
    }

    #[test]
    fn invalid_numeric_session_options_return_field_specific_errors() {
        type Mutate = fn(&mut SessionOptions);
        let cases: &[(&str, Mutate)] = &[
            ("auto_compact_threshold", |opts| {
                opts.auto_compact_threshold = Some(f32::NAN)
            }),
            ("auto_compact_threshold", |opts| {
                opts.auto_compact_threshold = Some(-0.01)
            }),
            ("auto_compact_threshold", |opts| {
                opts.auto_compact_threshold = Some(1.01)
            }),
            ("max_context_tokens", |opts| {
                opts.max_context_tokens = Some(0)
            }),
            ("temperature", |opts| opts.temperature = Some(f32::INFINITY)),
            ("temperature", |opts| opts.temperature = Some(-0.01)),
            ("temperature", |opts| opts.temperature = Some(1.01)),
            ("tool_timeout_ms", |opts| opts.tool_timeout_ms = Some(0)),
            ("llm_api_timeout_ms", |opts| {
                opts.llm_api_timeout_ms = Some(0)
            }),
            ("max_execution_time_ms", |opts| {
                opts.max_execution_time_ms = Some(0)
            }),
            ("circuit_breaker_threshold", |opts| {
                opts.circuit_breaker_threshold = Some(0)
            }),
            ("duplicate_tool_call_threshold", |opts| {
                opts.duplicate_tool_call_threshold = Some(0)
            }),
            ("max_tool_rounds", |opts| opts.max_tool_rounds = Some(0)),
            ("max_parallel_tasks", |opts| {
                opts.max_parallel_tasks = Some(0)
            }),
            ("llm_top_logprobs", |opts| opts.llm_top_logprobs = Some(21)),
        ];

        for (expected_field, mutate) in cases {
            let mut options = SessionOptions::new().with_session_id("numeric-validation");
            mutate(&mut options);
            let error = validate_session_options(&options).unwrap_err();
            assert!(
                matches!(
                    error,
                    CodeError::SessionConfiguration { field, .. } if field == *expected_field
                ),
                "expected field {expected_field}, got {error:?}"
            );
        }
    }

    #[test]
    fn invalid_structural_session_options_return_field_specific_errors() {
        let mut missing_id = SessionOptions::new();
        let error = validate_session_options(&missing_id).unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "session_id",
                ..
            }
        ));

        missing_id.session_id = Some(" \t".to_string());
        let error = validate_session_options(&missing_id).unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "session_id",
                ..
            }
        ));

        let mut conflicting_memory = SessionOptions::new().with_session_id("conflicting-memory");
        conflicting_memory.memory_store = Some(Arc::new(a3s_memory::InMemoryStore::new()));
        conflicting_memory.file_memory_dir = Some(PathBuf::from("memory"));
        let error = validate_session_options(&conflicting_memory).unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "memory_store",
                ..
            }
        ));

        let mut conflicting_session_store =
            SessionOptions::new().with_session_id("conflicting-session-store");
        conflicting_session_store.session_store =
            Some(Arc::new(crate::store::MemorySessionStore::new()));
        conflicting_session_store.file_session_store_dir = Some(PathBuf::from("sessions"));
        let error = validate_session_options(&conflicting_session_store).unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "session_store",
                ..
            }
        ));

        let mut invalid_policy = crate::tools::ToolResultTransformPolicyV1::context_efficient();
        invalid_policy.schema = "future".to_string();
        let options = SessionOptions::new()
            .with_session_id("invalid-transform-policy")
            .with_tool_result_transform_policy(invalid_policy);
        let error = validate_session_options(&options).unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "tool_result_transform_policy",
                ..
            }
        ));
    }

    #[test]
    fn numeric_session_option_boundaries_are_valid() {
        for threshold in [0.0, 1.0] {
            for temperature in [0.0, 1.0] {
                let mut options = SessionOptions::new().with_session_id("valid-boundaries");
                options.auto_compact_threshold = Some(threshold);
                options.max_context_tokens = Some(1);
                options.temperature = Some(temperature);
                options.tool_timeout_ms = Some(1);
                options.llm_api_timeout_ms = Some(1);
                options.max_execution_time_ms = Some(1);
                options.circuit_breaker_threshold = Some(1);
                options.duplicate_tool_call_threshold = Some(1);
                options.max_tool_rounds = Some(1);
                options.max_parallel_tasks = Some(1);
                options.llm_top_logprobs = Some(20);

                assert!(validate_session_options(&options).is_ok());
            }
        }
    }

    #[test]
    fn configured_model_context_window_drives_compaction_accounting() {
        let provider = serde_json::from_value(serde_json::json!({
            "name": "openai",
            "models": [{
                "id": "gpt-test",
                "limit": { "context": 128000 }
            }]
        }))
        .unwrap();
        let config = CodeConfig {
            default_model: Some("openai/gpt-test".to_string()),
            providers: vec![provider],
            ..Default::default()
        };

        assert_eq!(
            configured_model_context_tokens(&config, "openai/gpt-test"),
            Some(128_000)
        );
        assert_eq!(
            configured_model_context_tokens(&config, "openai/missing"),
            None
        );
    }
}
