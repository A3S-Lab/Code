use super::*;

// ============================================================================

#[napi(object)]
#[derive(Default)]
pub struct PermissionPolicy {
    /// Tool invocation patterns that are always denied first.
    pub deny: Option<Vec<String>>,
    /// Tool invocation patterns that are auto-approved.
    pub allow: Option<Vec<String>>,
    /// Tool invocation patterns that always require confirmation.
    pub ask: Option<Vec<String>>,
    /// Default decision when no rule matches: "allow", "deny", or "ask".
    pub default_decision: Option<String>,
    /// Whether this policy is enabled. Defaults to true.
    pub enabled: Option<bool>,
}

/// Reproducible recipe for a disposable worker/subagent.
///
/// This is the Node.js cattle-mode interface: define workers in data, pass them
/// to SessionOptions.workerAgents, Agent.sessionForWorker(), or
/// Session.registerWorkerAgent(). The Rust core compiles each spec into the
/// normal delegated-agent runtime definition.
#[napi(object)]
#[derive(Default)]
pub struct WorkerAgentSpec {
    /// Stable worker name used by task delegation.
    pub name: String,
    /// Human-readable worker purpose.
    pub description: String,
    /// Preset role: "read_only", "planner", "implementer", "verifier", "reviewer", or "custom".
    pub kind: Option<String>,
    /// Hide from UI lists while allowing explicit delegation.
    pub hidden: Option<bool>,
    /// Optional permission policy override.
    pub permissions: Option<PermissionPolicy>,
    /// Optional model override in "provider/model" format.
    pub model: Option<String>,
    /// Optional worker-specific prompt.
    pub prompt: Option<String>,
    /// Maximum execution steps/tool rounds.
    pub max_steps: Option<u32>,
    /// How child runs resolve Ask decisions: "auto_approve" (default), "deny_on_ask", or "inherit_parent".
    pub confirmation_inheritance: Option<String>,
}

#[napi(object)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub native: bool,
    pub hidden: bool,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub max_steps: Option<u32>,
    /// How child runs resolve Ask decisions: "auto_approve", "deny_on_ask", or "inherit_parent".
    pub confirmation_inheritance: Option<String>,
}

/// HITL confirmation policy configuration.
///
/// Controls the runtime behavior of Human-in-the-Loop confirmation flow.
#[napi(object)]
#[derive(Default)]
pub struct ConfirmationPolicy {
    /// Whether HITL is enabled (default: false, all tools auto-approved).
    pub enabled: Option<bool>,
    /// Default timeout in milliseconds (default: 30000 = 30s).
    pub default_timeout_ms: Option<u32>,
    /// Action to take on timeout: "reject" or "auto_approve" (default: "reject").
    pub timeout_action: Option<String>,
    /// Lanes that should auto-approve without confirmation: "control", "query", "execute", or "generate".
    pub yolo_lanes: Option<Vec<String>>,
}

/// Snapshot of a pending HITL tool confirmation.
#[napi(object)]
pub struct PendingConfirmation {
    /// Tool call ID to pass to `confirmToolUse`.
    pub tool_id: String,
    /// Tool name awaiting confirmation.
    pub tool_name: String,
    /// Tool arguments for display in a confirmation UI.
    pub args: serde_json::Value,
    /// Milliseconds remaining before the confirmation times out.
    pub remaining_ms: f64,
}

impl From<a3s_code_core::hitl::PendingConfirmationInfo> for PendingConfirmation {
    fn from(info: a3s_code_core::hitl::PendingConfirmationInfo) -> Self {
        Self {
            tool_id: info.tool_id,
            tool_name: info.tool_name,
            args: info.args,
            remaining_ms: info.remaining_ms as f64,
        }
    }
}

#[napi(object)]
#[derive(Default)]
pub struct AutoDelegationOptions {
    /// Enable runtime-driven automatic child-agent delegation.
    pub enabled: Option<bool>,
    /// Allow automatic delegation to launch multiple child agents in parallel.
    ///
    /// Manual `task` fan-out and legacy `parallel_task` calls remain available
    /// when this is false.
    pub auto_parallel: Option<bool>,
    /// Minimum local confidence required to auto-delegate a child task.
    pub min_confidence: Option<f64>,
    /// Maximum number of automatic child tasks per user request.
    pub max_tasks: Option<u32>,
}

/// Host-provided deterministic ID and clock configuration.
///
/// Set both fields when replaying a run so session/run IDs and timestamps are
/// reproducible across hosts. Omitted fields keep their system-backed default.
#[napi(object)]
#[derive(Default)]
pub struct HostEnvOptions {
    /// Prefix for deterministic IDs (`<prefix>-0`, `<prefix>-1`, ...).
    pub sequential_id_prefix: Option<String>,
    /// Fixed Unix-epoch timestamp returned by the session clock.
    pub fixed_time_ms: Option<f64>,
}

#[napi(object)]
#[derive(Default)]
pub struct SessionOptions {
    /// Override the default model. Format: "provider/model" (e.g., "openai/gpt-4o").
    pub model: Option<String>,
    /// Global admission priority: urgent, interactive, foreground, background, or maintenance.
    pub task_priority: Option<String>,
    /// Compatibility flag for the built-in skill registry.
    ///
    /// A3S Code currently ships no embedded built-in skills; `true` requests
    /// the empty compatibility registry, while `false` leaves the default
    /// effective registry unchanged.
    pub builtin_skills: Option<bool>,
    /// Extra directories to scan for skill files (.md with YAML frontmatter).
    pub skill_dirs: Option<Vec<String>>,
    /// Whether active skill allowed-tools restrict ordinary session tool calls.
    ///
    /// Defaults to false. Set true to restore the legacy global active-skill
    /// restriction before permission policy, hooks, or HITL run.
    pub enforce_active_skill_tool_restrictions: Option<bool>,
    /// Extra directories to scan for agent files.
    pub agent_dirs: Option<Vec<String>>,
    /// Reproducible disposable workers to register for task delegation.
    pub worker_agents: Option<Vec<WorkerAgentSpec>>,
    /// Optional advanced queue configuration for explicit external/hybrid lane dispatch.
    ///
    /// Ordinary sessions are queue-free unless this is provided.
    pub queue_config: Option<SessionQueueConfig>,
    /// Explicit permission policy for tool execution.
    pub permission_policy: Option<PermissionPolicy>,
    /// Explicit planning mode: "auto", "enabled", or "disabled".
    ///
    /// Prefer this over `planning` when the caller needs an unambiguous SDK contract.
    /// If both are set, `planningMode` wins.
    pub planning_mode: Option<String>,
    /// Legacy planning shortcut. Omit for auto planning, true to force planning, false to disable.
    pub planning: Option<bool>,
    /// Enable goal tracking (default: false).
    pub goal_tracking: Option<bool>,
    /// Max consecutive parse errors before abort.
    pub max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    pub tool_timeout_ms: Option<f64>,
    /// Per-model API HTTP timeout in milliseconds.
    pub llm_api_timeout_ms: Option<f64>,
    /// Max LLM API failures before abort.
    pub circuit_breaker_threshold: Option<u32>,
    /// Max consecutive identical tool signatures before duplicate-call guard failure.
    pub duplicate_tool_call_threshold: Option<u32>,
    /// Enable auto-compaction when context window fills up (default: false).
    pub auto_compact: Option<bool>,
    /// Context usage threshold (0.0–1.0) to trigger auto-compaction (default: 0.8).
    pub auto_compact_threshold: Option<f64>,
    /// Active model context window used for auto-compaction accounting.
    pub max_context_tokens: Option<f64>,
    /// Retention limits for large tool/program artifacts.
    pub artifact_store_limits: Option<ArtifactStoreLimits>,
    /// Host-pinned deterministic policy for projecting large tool results.
    pub tool_result_transform_policy: Option<ToolResultTransformPolicy>,
    /// Long-term memory store backend override.
    ///
    /// Sessions resolve a default store when this is not set.
    ///
    /// Pass `new FileMemoryStore("./memory")` for file-based persistence.
    /// ```js
    /// agent.session('.', { memoryStore: new FileMemoryStore('./memory') });
    /// ```
    pub memory_store: Option<JsMemoryStore>,
    /// Session persistence store backend.
    ///
    /// Pass `new FileSessionStore("./sessions")` to persist sessions to disk,
    /// or `new MemorySessionStore()` for an ephemeral in-process store.
    /// ```js
    /// agent.session('.', {
    ///   sessionStore: new FileSessionStore('./sessions'),
    ///   sessionId: 'my-session',
    ///   autoSave: true,
    /// });
    /// ```
    pub session_store: Option<JsSessionStore>,
    /// Security provider.
    ///
    /// Pass `new DefaultSecurityProvider()` to enable input taint tracking and
    /// output sanitisation. Omit to disable security (default: no security).
    /// ```js
    /// agent.session('.', { securityProvider: new DefaultSecurityProvider() });
    /// ```
    pub security_provider: Option<JsSecurityProvider>,
    /// Workspace backend used by built-in tools.
    ///
    /// Pass `new LocalWorkspaceBackend("/repo")` to explicitly use the local
    /// filesystem backend. This option is the SDK surface for future remote,
    /// browser, DFS, and container-backed workspace implementations.
    /// ```js
    /// agent.session('/repo', { workspaceBackend: new LocalWorkspaceBackend('/repo') });
    /// ```
    pub workspace_backend: Option<JsWorkspaceBackend>,
    /// Session-bound asynchronous semantic workspace retrieval.
    ///
    /// Pass `new WorkspaceRetrievalOptions(provider)`. Index construction
    /// starts in the background and all vectors remain in memory for this session.
    #[napi(ts_type = "WorkspaceRetrievalOptions")]
    pub workspace_retrieval: Option<WorkspaceRetrievalOptionsObject>,
    /// Optional remote git provider. When set, the resulting session attaches
    /// a `RemoteGitBackend` on top of `workspaceBackend` so the built-in
    /// `git` tool is available even on object-storage workspaces.
    ///
    /// ```js
    /// agent.session('s3://workspace/u1/s1', {
    ///   workspaceBackend: new S3WorkspaceBackend({ ... }),
    ///   remoteGit: {
    ///     baseUrl: 'https://gitserver.internal',
    ///     repoId:  'u1/s1',
    ///     bearerToken: token,
    ///   },
    /// });
    /// ```
    pub remote_git: Option<JsRemoteGitBackendConfig>,
    /// Custom role/identity prepended before the core agentic prompt.
    /// Example: "You are a senior Python developer specializing in FastAPI."
    pub role: Option<String>,
    /// Custom coding guidelines appended after the core prompt.
    /// Example: "Always use type hints. Follow PEP 8."
    pub guidelines: Option<String>,
    /// Custom response style (replaces default Response Format section).
    pub response_style: Option<String>,
    /// Freeform extra instructions appended at the end.
    pub extra: Option<String>,
    /// Inline skills registered programmatically without needing skill files on disk.
    /// Each entry defines an instruction or persona skill injected into the system prompt.
    pub inline_skills: Option<Vec<InlineSkill>>,
    /// Override maximum number of tool-call rounds for this session.
    pub max_tool_rounds: Option<u32>,
    /// Override maximum sibling parallel branches for this session.
    pub max_parallel_tasks: Option<u32>,
    /// Override automatic child-agent delegation for this session.
    pub auto_delegation: Option<AutoDelegationOptions>,
    /// Global session-level kill switch for automatic parallel child-agent fan-out.
    ///
    /// Manual `task` fan-out and legacy `parallel_task` calls remain available
    /// when this is false.
    pub auto_parallel: Option<bool>,
    /// Session-level switch for the model-visible `task` tool and hidden
    /// `parallel_task` compatibility alias.
    pub manual_delegation_enabled: Option<bool>,
    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    /// Only applied when `model` is also set.
    pub temperature: Option<f64>,
    /// Extended thinking token budget (e.g. 10_000). Enables chain-of-thought reasoning.
    /// Only applied when `model` is also set. Provider must support extended thinking.
    pub thinking_budget: Option<u32>,
    /// Request token-level log probabilities from OpenAI-compatible backends.
    ///
    /// Providers that do not support logprobs may reject the request.
    pub llm_logprobs: Option<bool>,
    /// Number of top token log probabilities to request when logprobs are enabled.
    pub llm_top_logprobs: Option<u32>,
    /// Structured JSONL trajectory path.
    ///
    /// When set, records user input, LLM turns, tool calls, tool observations,
    /// token usage, and execution end status.
    pub trajectory_path: Option<String>,
    /// Trajectory mode: "on" or "off". Defaults to "on" when `trajectoryPath` is set.
    pub trajectory_mode: Option<String>,
    /// Max bytes retained for any single text field before truncation.
    pub trajectory_max_text_bytes: Option<u32>,
    /// Whether LLM request records include full message arrays.
    pub trajectory_include_messages: Option<bool>,
    /// Enable continuation injection (default: true).
    /// When enabled, the loop injects a follow-up prompt when the LLM stops without completing.
    pub continuation_enabled: Option<bool>,
    /// Maximum continuation injections per execution (default: 3).
    pub max_continuation_turns: Option<u32>,
    /// Session ID (auto-generated if not set).
    ///
    /// Set a stable ID so the session can be saved and resumed later:
    /// ```js
    /// agent.session('.', { sessionId: 'my-session', sessionStore: new FileSessionStore('./sessions'), autoSave: true });
    /// // Later:
    /// agent.resumeSession('my-session', { sessionStore: new FileSessionStore('./sessions') });
    /// ```
    pub session_id: Option<String>,
    /// Host-defined tenant id. Opaque to the framework — propagated to
    /// SessionData, hooks, and traces for multi-tenant aggregation /
    /// billing. Pair with `principal` / `agentTemplateId` /
    /// `correlationId` for full identity context.
    pub tenant_id: Option<String>,
    /// Identity of the principal (user / service / etc.) that triggered
    /// this session. Treated as opaque.
    pub principal: Option<String>,
    /// Logical identifier of the agent template / definition the session
    /// was instantiated from.
    pub agent_template_id: Option<String>,
    /// Distributed-trace correlation id propagated through this
    /// session's events.
    pub correlation_id: Option<String>,
    /// Deterministic ID and clock configuration for replay and tests.
    pub host_env: Option<HostEnvOptions>,
    /// Optional FIFO retention caps on the session's in-memory stores.
    /// Missing fields keep finite framework defaults. Set `unbounded: true`
    /// only when unlimited retention is deliberate.
    pub retention_limits: Option<RetentionLimitsObject>,
    /// Automatically save the session to the configured store after each turn (default: false).
    pub auto_save: Option<bool>,
    /// HITL confirmation policy configuration.
    ///
    /// Pass a confirmation policy to enable Human-in-the-Loop confirmation for tool execution.
    /// When enabled, tools that require confirmation will emit ConfirmationRequired events
    /// and wait for user approval before executing.
    ///
    /// ```js
    /// agent.session('.', {
    ///   confirmationPolicy: {
    ///     enabled: true,
    ///     defaultTimeoutMs: 30000,
    ///     timeoutAction: 'reject'
    ///   }
    /// });
    /// ```
    pub confirmation_policy: Option<ConfirmationPolicy>,
    /// Maximum execution time in milliseconds.
    ///
    /// When set, the execution loop will abort if it exceeds this duration.
    /// This prevents runaway executions and excessive API costs.
    ///
    /// ```js
    /// agent.session('.', {
    ///   maxExecutionTimeMs: 300000  // 5 minutes
    /// });
    /// ```
    pub max_execution_time_ms: Option<f64>,
}

/// Retention limits for large tool/program artifacts.
#[napi(object)]
#[derive(Clone)]
pub struct ArtifactStoreLimits {
    /// Maximum number of artifacts retained by a session.
    pub max_artifacts: Option<f64>,
    /// Maximum total artifact content bytes retained by a session.
    pub max_bytes: Option<f64>,
}

/// Versioned deterministic Tool-result projection policy.
#[napi(object)]
#[derive(Clone)]
pub struct ToolResultTransformPolicy {
    pub schema: String,
    pub max_output_bytes: f64,
    pub head_bytes: f64,
    pub tail_bytes: f64,
    pub fold_repeated_lines: bool,
    pub repeated_line_threshold: f64,
    pub structured_sample_items: f64,
}
#[napi(object)]
#[derive(Clone, Default)]
pub struct SessionQueueConfig {
    /// Max concurrency for Query lane (default: 4).
    pub query_concurrency: Option<u32>,
    /// Max concurrency for Execute lane (default: 2).
    pub execute_concurrency: Option<u32>,
    /// Max concurrency for Generate lane (default: 1).
    pub generate_concurrency: Option<u32>,
    /// Enable dead letter queue.
    pub enable_dlq: Option<bool>,
    /// Max DLQ size (default: 1000).
    pub dlq_max_size: Option<u32>,
    /// Enable metrics collection.
    pub enable_metrics: Option<bool>,
    /// Enable queue alerts.
    pub enable_alerts: Option<bool>,
    /// Default command timeout (ms).
    pub timeout_ms: Option<u32>,
    /// Enable all features with sensible defaults.
    pub enable_all_features: Option<bool>,
    /// Per-lane handler config. Keys: "control", "query", "execute", "generate".
    /// Values: LaneHandlerConfig with mode ("internal"/"external"/"hybrid") and timeoutMs.
    pub lane_handlers: Option<std::collections::HashMap<String, LaneHandlerConfig>>,
}

/// Result of an external task completion.
#[napi(object)]
#[derive(Clone)]
pub struct ExternalTaskResult {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Lane handler configuration.
#[napi(object)]
#[derive(Clone)]
pub struct LaneHandlerConfig {
    /// "internal", "external", or "hybrid"
    pub mode: String,
    /// Timeout for external processing (ms).
    pub timeout_ms: Option<u32>,
}

/// Queue statistics.
#[napi(object)]
#[derive(Clone)]
pub struct QueueStats {
    pub total_pending: u32,
    pub total_active: u32,
    pub external_pending: u32,
}

pub(super) fn js_queue_config_to_rust(
    config: &SessionQueueConfig,
) -> napi::Result<RustSessionQueueConfig> {
    let mut c = if config.enable_all_features.unwrap_or(false) {
        RustSessionQueueConfig::default().with_lane_features()
    } else {
        RustSessionQueueConfig::default()
    };
    if let Some(n) = config.query_concurrency {
        c.query_max_concurrency = n as usize;
    }
    if let Some(n) = config.execute_concurrency {
        c.execute_max_concurrency = n as usize;
    }
    if let Some(n) = config.generate_concurrency {
        c.generate_max_concurrency = n as usize;
    }
    if let Some(true) = config.enable_dlq {
        c = c.with_dlq(config.dlq_max_size.map(|n| n as usize));
    }
    if let Some(true) = config.enable_metrics {
        c = c.with_metrics();
    }
    if let Some(true) = config.enable_alerts {
        c = c.with_alerts();
    }
    if let Some(ms) = config.timeout_ms {
        c = c.with_timeout(ms as u64);
    }
    if let Some(ref handlers) = config.lane_handlers {
        for (lane_str, handler) in handlers {
            let lane = parse_lane(lane_str)?;
            let mode = parse_handler_mode(&handler.mode)?;
            let lane_cfg = RustLaneHandlerConfig {
                mode,
                timeout_ms: handler.timeout_ms.map(|ms| ms as u64).unwrap_or(60_000),
            };
            c.lane_handlers.insert(lane, lane_cfg);
        }
    }
    Ok(c)
}

pub(super) fn parse_lane(lane: &str) -> napi::Result<RustSessionLane> {
    match lane.trim().to_ascii_lowercase().as_str() {
        "control" => Ok(RustSessionLane::Control),
        "query" => Ok(RustSessionLane::Query),
        "execute" => Ok(RustSessionLane::Execute),
        "generate" => Ok(RustSessionLane::Generate),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid lane '{}'. Must be: control, query, execute, or generate",
            lane
        ))),
    }
}

pub(super) fn parse_handler_mode(mode: &str) -> napi::Result<RustTaskHandlerMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "internal" => Ok(RustTaskHandlerMode::Internal),
        "external" => Ok(RustTaskHandlerMode::External),
        "hybrid" => Ok(RustTaskHandlerMode::Hybrid),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid handler mode '{}'. Must be: internal, external, or hybrid",
            mode
        ))),
    }
}

pub(super) fn js_optional_usize(
    value: Option<f64>,
    field_name: &str,
    default_value: usize,
) -> napi::Result<usize> {
    match value {
        Some(n)
            if n.is_finite()
                && (0.0..=9_007_199_254_740_991.0).contains(&n)
                && n.fract() == 0.0 =>
        {
            Ok(n as usize)
        }
        Some(_) => Err(napi::Error::from_reason(format!(
            "{field_name} must be a non-negative integer"
        ))),
        None => Ok(default_value),
    }
}

pub(super) fn js_artifact_store_limits_to_rust(
    limits: ArtifactStoreLimits,
) -> napi::Result<a3s_code_core::tools::ArtifactStoreLimits> {
    let defaults = a3s_code_core::tools::ArtifactStoreLimits::default();
    Ok(a3s_code_core::tools::ArtifactStoreLimits {
        max_artifacts: js_optional_usize(
            limits.max_artifacts,
            "artifactStoreLimits.maxArtifacts",
            defaults.max_artifacts,
        )?,
        max_bytes: js_optional_usize(
            limits.max_bytes,
            "artifactStoreLimits.maxBytes",
            defaults.max_bytes,
        )?,
    })
}

pub(super) fn js_tool_result_transform_policy_to_rust(
    policy: ToolResultTransformPolicy,
) -> napi::Result<a3s_code_core::tools::ToolResultTransformPolicyV1> {
    Ok(a3s_code_core::tools::ToolResultTransformPolicyV1 {
        schema: policy.schema,
        max_output_bytes: js_optional_usize(
            Some(policy.max_output_bytes),
            "toolResultTransformPolicy.maxOutputBytes",
            0,
        )?,
        head_bytes: js_optional_usize(
            Some(policy.head_bytes),
            "toolResultTransformPolicy.headBytes",
            0,
        )?,
        tail_bytes: js_optional_usize(
            Some(policy.tail_bytes),
            "toolResultTransformPolicy.tailBytes",
            0,
        )?,
        fold_repeated_lines: policy.fold_repeated_lines,
        repeated_line_threshold: js_optional_usize(
            Some(policy.repeated_line_threshold),
            "toolResultTransformPolicy.repeatedLineThreshold",
            0,
        )?,
        structured_sample_items: js_optional_usize(
            Some(policy.structured_sample_items),
            "toolResultTransformPolicy.structuredSampleItems",
            0,
        )?,
    })
}
pub(super) fn js_auto_delegation_to_rust(
    options: AutoDelegationOptions,
) -> a3s_code_core::AutoDelegationConfig {
    let mut config = a3s_code_core::AutoDelegationConfig::default();
    if let Some(enabled) = options.enabled {
        config.enabled = enabled;
    }
    if let Some(auto_parallel) = options.auto_parallel {
        config.auto_parallel = auto_parallel;
    }
    if let Some(min_confidence) = options.min_confidence {
        config.min_confidence = (min_confidence as f32).clamp(0.0, 1.0);
    }
    if let Some(max_tasks) = options.max_tasks {
        config.max_tasks = (max_tasks as usize).max(1);
    }
    config
}

/// Build RustSessionOptions from JS SessionOptions.
pub(super) fn js_session_options_to_rust(
    options: Option<SessionOptions>,
) -> napi::Result<RustSessionOptions> {
    let Some(o) = options else {
        return Ok(RustSessionOptions::new());
    };
    let mut opts = RustSessionOptions::new();
    if let Some(model) = o.model {
        opts = opts.with_model(model);
    }
    if let Some(priority) = o.task_priority {
        opts = opts.with_task_priority(
            priority
                .parse()
                .map_err(|error| napi::Error::from_reason(format!("taskPriority: {error}")))?,
        );
    }
    if o.builtin_skills.unwrap_or(false) {
        opts = opts.with_builtin_skills();
    }
    if let Some(dirs) = o.skill_dirs {
        for d in dirs {
            opts = opts.with_skills_from_dir(d);
        }
    }
    if let Some(enabled) = o.enforce_active_skill_tool_restrictions {
        opts = opts.with_active_skill_tool_restrictions(enabled);
    }
    if let Some(dirs) = o.agent_dirs {
        for d in dirs {
            opts = opts.with_agent_dir(d);
        }
    }
    if let Some(workers) = o.worker_agents {
        for worker in workers {
            opts = opts.with_worker_agent(js_worker_agent_spec_to_rust(worker)?);
        }
    }
    if let Some(qc) = o.queue_config {
        opts = opts.with_queue_config(js_queue_config_to_rust(&qc)?);
    }
    if let Some(policy) = o.permission_policy {
        opts = opts.with_permission_policy(js_permission_policy_to_rust(policy)?);
    }
    opts = apply_planning_mode(opts, o.planning_mode.as_deref(), o.planning)?;
    if o.goal_tracking.unwrap_or(false) {
        opts = opts.with_goal_tracking(true);
    }
    if let Some(n) = o.max_parse_retries {
        opts = opts.with_parse_retries(n);
    }
    if let Some(ms) = o.tool_timeout_ms {
        opts = opts.with_tool_timeout(ms as u64);
    }
    if let Some(ms) = o.llm_api_timeout_ms {
        opts = opts.with_llm_api_timeout(ms as u64);
    }
    if let Some(n) = o.circuit_breaker_threshold {
        opts = opts.with_circuit_breaker(n);
    }
    if let Some(n) = o.duplicate_tool_call_threshold {
        opts = opts.with_duplicate_tool_call_threshold(n);
    }
    if o.auto_compact.unwrap_or(false) {
        opts = opts.with_auto_compact(true);
    }
    if let Some(t) = o.auto_compact_threshold {
        opts = opts.with_auto_compact_threshold(t as f32);
    }
    if let Some(tokens) = o.max_context_tokens {
        let tokens = js_optional_usize(Some(tokens), "maxContextTokens", 0)?;
        if tokens == 0 {
            return Err(napi::Error::from_reason(
                "maxContextTokens must be a positive integer".to_string(),
            ));
        }
        opts = opts.with_max_context_tokens(tokens);
    }
    if let Some(limits) = o.artifact_store_limits {
        opts = opts.with_artifact_store_limits(js_artifact_store_limits_to_rust(limits)?);
    }
    if let Some(policy) = o.tool_result_transform_policy {
        opts = opts
            .with_tool_result_transform_policy(js_tool_result_transform_policy_to_rust(policy)?);
    }
    if let Some(ref store) = o.memory_store {
        if store.backend == "file" {
            if let Some(ref dir) = store.dir {
                opts = opts.with_file_memory(dir);
            }
        }
    }
    if let Some(ref store) = o.session_store {
        match store.backend.as_str() {
            "file" => {
                if let Some(ref dir) = store.dir {
                    opts = opts.with_file_session_store(dir);
                }
            }
            "memory" => {
                let memory_store = resolve_node_memory_session_store(store.instance_id.as_deref())?;
                let s: std::sync::Arc<dyn a3s_code_core::store::SessionStore> = memory_store;
                opts = opts.with_session_store(s);
            }
            _ => {}
        }
    }
    if let Some(ref sec) = o.security_provider {
        if sec.kind.is_empty() || sec.kind == "default" {
            opts = opts.with_default_security();
        }
    }
    if let Some(ref backend) = o.workspace_backend {
        let services: std::sync::Arc<a3s_code_core::WorkspaceServices> = match backend.kind.as_str()
        {
            "" | "local" => {
                let root = backend.root.as_ref().ok_or_else(|| {
                    napi::Error::from_reason("LocalWorkspaceBackend requires a root path")
                })?;
                a3s_code_core::WorkspaceServices::local(root.clone())
            }
            "s3" => {
                let s3_config = backend.s3.as_ref().ok_or_else(|| {
                    napi::Error::from_reason(
                        "S3WorkspaceBackend requires the `s3` configuration field",
                    )
                })?;
                a3s_code_core::WorkspaceServices::s3(s3_config_to_core(s3_config))
            }
            other => {
                return Err(napi::Error::from_reason(format!(
                    "Unsupported workspace backend kind '{other}'"
                )));
            }
        };
        let services = if let Some(ref git_cfg) = o.remote_git {
            services
                .with_remote_git(remote_git_config_to_core(git_cfg))
                .map_err(|e| napi::Error::from_reason(format!("with_remote_git: {e}")))?
        } else {
            services
        };
        opts = opts.with_workspace_backend(services);
    } else if o.remote_git.is_some() {
        // `remoteGit` needs a base `WorkspaceServices` to attach to. The
        // session path is not available here (it's the first argument to
        // `agent.session(path, options)`, applied later by the runtime),
        // so we cannot synthesize a local backend on the user's behalf.
        return Err(napi::Error::from_reason(
            "remoteGit requires workspaceBackend to be set; pass a LocalWorkspaceBackend or S3WorkspaceBackend alongside it",
        ));
    }
    if let Some(ref retrieval) = o.workspace_retrieval {
        opts = opts.with_workspace_retrieval(js_workspace_retrieval_to_rust(retrieval)?);
    }
    // Build prompt slots if any slot is set
    if o.role.is_some() || o.guidelines.is_some() || o.response_style.is_some() || o.extra.is_some()
    {
        let slots = a3s_code_core::SystemPromptSlots {
            style: None,
            role: o.role,
            guidelines: o.guidelines,
            response_style: o.response_style,
            extra: o.extra,
        };
        opts = opts.with_prompt_slots(slots);
    }
    // Inline skills registered without skill files
    if let Some(inline_skills) = o.inline_skills {
        if !inline_skills.is_empty() {
            let registry = a3s_code_core::skills::SkillRegistry::new();
            for skill in inline_skills {
                registry.register_unchecked(inline_skill_to_rust(skill)?);
            }
            opts = opts.with_skill_registry(std::sync::Arc::new(registry));
        }
    }
    if let Some(r) = o.max_tool_rounds {
        opts = opts.with_max_tool_rounds(r as usize);
    }
    if let Some(max_parallel_tasks) = o.max_parallel_tasks {
        opts = opts.with_max_parallel_tasks(max_parallel_tasks as usize);
    }
    if let Some(auto_delegation) = o.auto_delegation {
        opts = opts.with_auto_delegation(js_auto_delegation_to_rust(auto_delegation));
    }
    if let Some(auto_parallel) = o.auto_parallel {
        opts = opts.with_auto_parallel_delegation(auto_parallel);
    }
    if let Some(manual_delegation_enabled) = o.manual_delegation_enabled {
        opts = opts.with_manual_delegation_enabled(manual_delegation_enabled);
    }
    if let Some(id) = o.session_id {
        opts = opts.with_session_id(id);
    }
    if let Some(t) = o.tenant_id {
        opts = opts.with_tenant_id(t);
    }
    if let Some(p) = o.principal {
        opts = opts.with_principal(p);
    }
    if let Some(t) = o.agent_template_id {
        opts = opts.with_agent_template_id(t);
    }
    if let Some(c) = o.correlation_id {
        opts = opts.with_correlation_id(c);
    }
    if let Some(host_env) = o.host_env {
        opts = opts.with_host_env(js_host_env_to_rust(host_env)?);
    }
    if let Some(rl) = o.retention_limits {
        let mut limits = if rl.unbounded.unwrap_or(false) {
            a3s_code_core::retention::SessionRetentionLimits::unbounded()
        } else {
            a3s_code_core::retention::SessionRetentionLimits::default()
        };
        if let Some(n) = rl.max_runs_retained {
            limits.max_runs_retained = Some(n as usize);
        }
        if let Some(n) = rl.max_events_per_run {
            limits.max_events_per_run = Some(n as usize);
        }
        if let Some(n) = rl.max_event_bytes_per_run {
            limits.max_event_bytes_per_run = Some(n as usize);
        }
        if let Some(n) = rl.max_trace_events {
            limits.max_trace_events = Some(n as usize);
        }
        if let Some(n) = rl.max_terminal_subagent_tasks {
            limits.max_terminal_subagent_tasks = Some(n as usize);
        }
        opts = opts.with_retention_limits(limits);
    }
    if o.auto_save.unwrap_or(false) {
        opts = opts.with_auto_save(true);
    }
    if let Some(t) = o.temperature {
        opts = opts.with_temperature(t as f32);
    }
    if let Some(budget) = o.thinking_budget {
        opts = opts.with_thinking_budget(budget as usize);
    }
    if let Some(enabled) = o.llm_logprobs {
        opts = opts.with_llm_logprobs(enabled);
    }
    if let Some(top_logprobs) = o.llm_top_logprobs {
        opts = opts.with_llm_top_logprobs(top_logprobs as usize);
    }
    if let Some(path) = o.trajectory_path {
        let mut config = a3s_code_core::RlTrajectoryConfig::new(path);
        if let Some(mode) = o.trajectory_mode {
            let parsed = a3s_code_core::RlTrajectoryMode::parse(&mode).ok_or_else(|| {
                napi::Error::from_reason(format!(
                    "trajectoryMode must be 'on' or 'off', got {mode}"
                ))
            })?;
            config = config.with_mode(parsed);
        }
        if let Some(max_bytes) = o.trajectory_max_text_bytes {
            config = config.with_max_text_bytes(max_bytes as usize);
        }
        if let Some(include_messages) = o.trajectory_include_messages {
            config = config.with_include_messages(include_messages);
        }
        opts = opts.with_rl_trajectory(config);
    }
    if let Some(enabled) = o.continuation_enabled {
        opts = opts.with_continuation(enabled);
    }
    if let Some(turns) = o.max_continuation_turns {
        opts = opts.with_max_continuation_turns(turns);
    }

    // HITL confirmation policy configuration
    if let Some(policy) = o.confirmation_policy {
        opts = opts.with_confirmation_policy(js_confirmation_policy_to_rust(policy)?);
    }

    // Maximum execution time configuration
    if let Some(timeout_ms) = o.max_execution_time_ms {
        opts.max_execution_time_ms = Some(timeout_ms as u64);
    }

    Ok(opts)
}

fn js_host_env_to_rust(
    options: HostEnvOptions,
) -> napi::Result<std::sync::Arc<a3s_code_core::host_env::HostEnv>> {
    use a3s_code_core::host_env::{
        Clock, FixedClock, HostEnv, IdGenerator, SequentialIdGenerator, SystemClock,
        SystemIdGenerator,
    };

    let id_generator: std::sync::Arc<dyn IdGenerator> =
        if let Some(prefix) = options.sequential_id_prefix {
            std::sync::Arc::new(SequentialIdGenerator::new(prefix))
        } else {
            std::sync::Arc::new(SystemIdGenerator)
        };

    let clock: std::sync::Arc<dyn Clock> = if let Some(value) = options.fixed_time_ms {
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u64::MAX as f64 {
            return Err(napi::Error::from_reason(
                "hostEnv.fixedTimeMs must be a non-negative integer within the u64 range",
            ));
        }
        std::sync::Arc::new(FixedClock::new(value as u64))
    } else {
        std::sync::Arc::new(SystemClock)
    };

    Ok(std::sync::Arc::new(HostEnv::new(id_generator, clock)))
}

pub(super) fn apply_planning_mode(
    opts: RustSessionOptions,
    planning_mode: Option<&str>,
    planning: Option<bool>,
) -> napi::Result<RustSessionOptions> {
    if let Some(mode) = planning_mode {
        return Ok(opts.with_planning_mode(parse_planning_mode(mode)?));
    }

    if let Some(enabled) = planning {
        Ok(opts.with_planning(enabled))
    } else {
        Ok(opts)
    }
}

pub(super) fn parse_planning_mode(mode: &str) -> napi::Result<RustPlanningMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(RustPlanningMode::Auto),
        "enabled" | "enable" | "on" | "force" | "forced" | "true" => Ok(RustPlanningMode::Enabled),
        "disabled" | "disable" | "off" | "false" => Ok(RustPlanningMode::Disabled),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid planningMode '{}'. Must be: auto, enabled, or disabled",
            mode
        ))),
    }
}

pub(super) fn parse_permission_decision(
    value: Option<String>,
) -> napi::Result<RustPermissionDecision> {
    match value
        .as_deref()
        .unwrap_or("ask")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "allow" => Ok(RustPermissionDecision::Allow),
        "deny" => Ok(RustPermissionDecision::Deny),
        "ask" => Ok(RustPermissionDecision::Ask),
        other => Err(napi::Error::from_reason(format!(
            "Invalid permission defaultDecision '{}'. Must be: allow, deny, or ask",
            other
        ))),
    }
}

pub(super) fn parse_timeout_action(value: Option<&str>) -> napi::Result<RustTimeoutAction> {
    match value
        .unwrap_or("reject")
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "reject" => Ok(RustTimeoutAction::Reject),
        "auto_approve" | "autoapprove" => Ok(RustTimeoutAction::AutoApprove),
        other => Err(napi::Error::from_reason(format!(
            "Invalid confirmation timeoutAction '{}'. Must be: reject or auto_approve",
            other
        ))),
    }
}

pub(super) fn js_confirmation_policy_to_rust(
    policy: ConfirmationPolicy,
) -> napi::Result<RustConfirmationPolicy> {
    let mut rust_policy = if policy.enabled.unwrap_or(false) {
        RustConfirmationPolicy::enabled()
    } else {
        RustConfirmationPolicy::default()
    };

    if let Some(timeout_ms) = policy.default_timeout_ms {
        rust_policy = rust_policy.with_timeout(
            timeout_ms as u64,
            parse_timeout_action(policy.timeout_action.as_deref())?,
        );
    } else {
        parse_timeout_action(policy.timeout_action.as_deref())?;
    }

    if let Some(lanes) = policy.yolo_lanes {
        let yolo_lanes = lanes
            .iter()
            .map(|lane| parse_lane(lane))
            .collect::<napi::Result<Vec<_>>>()?;
        if !yolo_lanes.is_empty() {
            rust_policy = rust_policy.with_yolo_lanes(yolo_lanes);
        }
    }

    Ok(rust_policy)
}

pub(super) fn js_permission_policy_to_rust(
    policy: PermissionPolicy,
) -> napi::Result<RustPermissionPolicy> {
    Ok(RustPermissionPolicy {
        deny: policy
            .deny
            .unwrap_or_default()
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        allow: policy
            .allow
            .unwrap_or_default()
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        ask: policy
            .ask
            .unwrap_or_default()
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        default_decision: parse_permission_decision(policy.default_decision)?,
        enabled: policy.enabled.unwrap_or(true),
    })
}

pub(super) fn js_worker_agent_spec_to_rust(
    spec: WorkerAgentSpec,
) -> napi::Result<RustWorkerAgentSpec> {
    if spec.name.trim().is_empty() {
        return Err(napi::Error::from_reason("worker agent name is required"));
    }
    if spec.description.trim().is_empty() {
        return Err(napi::Error::from_reason(
            "worker agent description is required",
        ));
    }

    let kind = parse_worker_agent_kind(spec.kind.as_deref())?;
    let mut worker = RustWorkerAgentSpec::new(kind, spec.name, spec.description);
    if spec.hidden.unwrap_or(false) {
        worker = worker.hidden(true);
    }
    if let Some(policy) = spec.permissions {
        worker = worker.with_permissions(js_permission_policy_to_rust(policy)?);
    }
    if let Some(model) = spec.model {
        worker = worker.with_model(RustAgentModelConfig::from_model_ref(model));
    }
    if let Some(prompt) = spec.prompt {
        worker = worker.with_prompt(prompt);
    }
    if let Some(max_steps) = spec.max_steps {
        worker = worker.with_max_steps(max_steps as usize);
    }
    if let Some(ci) = spec.confirmation_inheritance {
        worker = worker.with_confirmation(parse_confirmation_inheritance(&ci)?);
    }
    Ok(worker)
}

pub(super) fn parse_worker_agent_kind(kind: Option<&str>) -> napi::Result<RustWorkerAgentKind> {
    kind.unwrap_or("custom")
        .parse::<RustWorkerAgentKind>()
        .map_err(|e| napi::Error::from_reason(e.to_string()))
}

pub(super) fn parse_confirmation_inheritance(
    value: &str,
) -> napi::Result<a3s_code_core::subagent::ConfirmationInheritance> {
    use a3s_code_core::subagent::ConfirmationInheritance;
    match value {
        "auto_approve" => Ok(ConfirmationInheritance::AutoApprove),
        "deny_on_ask" => Ok(ConfirmationInheritance::DenyOnAsk),
        "inherit_parent" => Ok(ConfirmationInheritance::InheritParent),
        other => Err(napi::Error::from_reason(format!(
            "invalid confirmation_inheritance: '{}' (expected: auto_approve, deny_on_ask, inherit_parent)",
            other
        ))),
    }
}

pub(super) fn confirmation_inheritance_to_js(
    ci: &a3s_code_core::subagent::ConfirmationInheritance,
) -> String {
    use a3s_code_core::subagent::ConfirmationInheritance;
    match ci {
        ConfirmationInheritance::AutoApprove => "auto_approve".to_string(),
        ConfirmationInheritance::DenyOnAsk => "deny_on_ask".to_string(),
        ConfirmationInheritance::InheritParent => "inherit_parent".to_string(),
    }
}

pub(super) fn rust_agent_definition_to_js(def: RustAgentDefinition) -> AgentDefinition {
    AgentDefinition {
        name: def.name,
        description: def.description,
        native: def.native,
        hidden: def.hidden,
        model: def.model.map(|model| model.model_ref()),
        prompt: def.prompt,
        max_steps: def.max_steps.map(|steps| steps as u32),
        confirmation_inheritance: def
            .confirmation_inheritance
            .as_ref()
            .map(confirmation_inheritance_to_js),
    }
}
