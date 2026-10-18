use super::{AgentConfig, ModelMiddlewareHealthSnapshot, ModelMiddlewareObs};
use crate::llm::{LlmClient, ModelGenerationAdmission};
use crate::loop_checkpoint::LoopCheckpointSink;
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::{ToolContext, ToolExecutor};
use std::sync::Arc;

impl crate::agent::TurnEngine {
    pub(crate) fn llm_api_timeout(&self) -> Option<std::time::Duration> {
        self.config
            .llm_api_timeout_ms
            .map(|timeout_ms| std::time::Duration::from_millis(timeout_ms.max(1)))
    }

    pub(crate) fn permission_checker(
        &self,
    ) -> Option<Arc<dyn crate::permissions::PermissionChecker>> {
        self.config.permission_checker.clone()
    }

    /// The host budget authority captured by this Run's loop configuration.
    pub(crate) fn tool_budget_guard(&self) -> Option<Arc<dyn crate::budget::BudgetGuard>> {
        self.config.budget_guard.clone()
    }

    /// Safety gate bound to this loop's configuration, for callers outside the
    /// agent module tree that must enforce the same tool-owned escalation rules.
    pub(crate) fn tool_safety_gate(&self) -> crate::safety_gate::ToolSafetyGate<'_> {
        crate::safety_gate::ToolSafetyGate::new(&self.config)
    }

    /// The HITL confirmation authority configured for this loop, if any.
    pub(crate) fn confirmation_provider(
        &self,
    ) -> Option<Arc<dyn crate::hitl::ConfirmationProvider>> {
        self.config.confirmation_manager.clone()
    }

    /// `Some` when this call is refused before any process exists.
    pub(crate) fn execution_unavailable(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<String> {
        let tool = self.tool_executor.registry().get(name)?;
        tool.validate_execution_available(args, &self.tool_context)
            .err()
            .map(|error| error.to_string())
    }

    pub(crate) fn sanitize_tool_output(&self, text: &str) -> String {
        match self.config.security_provider.as_ref() {
            Some(provider) => provider.sanitize_output(text),
            None => text.to_string(),
        }
    }

    /// Character budget for the fact-log compactor.
    pub(crate) fn fact_compact_after_chars(&self) -> usize {
        super::fact_compact_char_budget(
            self.config.auto_compact,
            self.config.max_context_tokens,
            self.config.auto_compact_threshold,
        )
    }

    /// Provider-token watermark. `None` when auto-compact is off.
    pub(crate) fn fact_compact_after_tokens(&self) -> Option<u64> {
        super::fact_compact_after_tokens(
            self.config.auto_compact,
            self.config.max_context_tokens,
            self.config.auto_compact_threshold,
        )
    }

    /// Recent transcript retained after a fact-log compaction.
    pub(crate) fn fact_compact_keep_tokens(&self) -> usize {
        super::fact_compact_keep_tokens(self.config.max_context_tokens)
    }

    pub(crate) fn duplicate_tool_threshold(&self) -> u32 {
        self.config.duplicate_tool_call_threshold
    }

    pub(crate) fn context_window_tokens(&self) -> usize {
        self.config.max_context_tokens
    }

    pub(crate) fn skill_restriction_denial(&self, name: &str) -> Option<(String, String)> {
        match crate::safety_gate::ToolSafetyGate::new(&self.config).check_skill_restrictions(name) {
            Some(crate::safety_gate::ToolGateDecision::Deny {
                output,
                event_reason,
                ..
            }) => Some((output, event_reason)),
            _ => None,
        }
    }

    pub(crate) fn new(
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        mut tool_context: ToolContext,
        config: AgentConfig,
    ) -> Self {
        let model_generation_admission =
            ModelGenerationAdmission::new(llm_client.model_generation_concurrency());
        tool_context.run_work_policy = config.work_policy.clone();
        Self {
            llm_client,
            model_generation_admission,
            shared_model_generation_admission: false,
            middleware_obs: ModelMiddlewareObs::shared(),
            tool_executor,
            tool_context,
            config,
            command_queue: None,
            checkpoint_sink: None,
            checkpoint_run_id: None,
            checkpoint_capability_binding: None,
            bound_invocation: None,
            capability_runtime: None,
            fact_turn: Arc::new(tokio::sync::Mutex::new(None)),
            child_run: false,
        }
    }

    /// Mark this loop as the engine of a `task` or `skill` child run.
    pub(crate) fn into_child_run(mut self) -> Self {
        self.child_run = true;
        self
    }

    pub(crate) fn is_child_run(&self) -> bool {
        self.child_run
    }

    /// Reuse the provider admission gate owned by the surrounding session.
    ///
    /// `AgentLoop` instances are rebuilt for each host-direct call so they can
    /// snapshot live tools and governance. Model-generation capacity is a
    /// provider/session contract, not a per-call resource, and therefore must
    /// survive those rebuilds.
    pub(crate) fn with_model_generation_admission(
        mut self,
        admission: ModelGenerationAdmission,
    ) -> Self {
        self.model_generation_admission = admission;
        self.shared_model_generation_admission = true;
        self
    }

    /// Reuse the middleware observation window owned by the surrounding session.
    ///
    /// Loops are rebuilt per host-direct call; stage counters must survive those
    /// rebuilds so hosts can inspect one secret-free health snapshot per session.
    pub(crate) fn with_model_middleware_obs(
        mut self,
        middleware_obs: Arc<ModelMiddlewareObs>,
    ) -> Self {
        self.middleware_obs = middleware_obs;
        self
    }

    /// Return secret-free middleware stage counters for this loop's observation window.
    ///
    /// The session facade reads the shared obs Arc directly; unit tests still
    /// query health through [`AgentLoop`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn model_middleware_health(&self) -> ModelMiddlewareHealthSnapshot {
        self.middleware_obs.snapshot()
    }

    pub(crate) fn with_capability_runtime(
        mut self,
        runtime: crate::capability::AgentCapabilityRuntime,
    ) -> Self {
        self.capability_runtime = Some(runtime);
        self
    }

    pub(crate) fn begin_capability_operation(
        &self,
        logical_turn: usize,
        fallback_cancellation: &tokio_util::sync::CancellationToken,
        label: &'static str,
    ) -> anyhow::Result<crate::capability::AgentCapabilityOperation> {
        crate::capability::AgentCapabilityOperation::begin(
            self.capability_runtime.as_ref(),
            logical_turn,
            fallback_cancellation,
            label,
        )
        .map_err(Into::into)
    }

    /// Set the lane queue for priority-based tool execution.
    ///
    /// When set, tools are routed through the lane queue which supports
    /// External task handling for multi-machine parallel processing.
    pub fn with_queue(mut self, queue: Arc<SessionLaneQueue>) -> Self {
        self.command_queue = Some(queue);
        self
    }

    /// Attach a per-tool-round checkpoint sink. After each completed
    /// tool round the loop will call `sink.save_checkpoint(...)`.
    ///
    /// The sink is independent from the run id: call
    /// [`AgentLoop::set_checkpoint_run`] before executing to bind the
    /// run id this execution will use.
    #[allow(dead_code)]
    pub fn with_checkpoint_sink(mut self, sink: Arc<dyn LoopCheckpointSink>) -> Self {
        self.checkpoint_sink = Some(sink);
        self
    }

    /// Bind the broadcast lane tools use to surface host-facing lifecycle
    /// events (subagent start/progress/end, confirmations, questions).
    /// Conversation fact streams inject one per stream so those events are
    /// relayed to the host channel instead of being dropped; without it the
    /// tool context carries no lane and delegated subagent telemetry never
    /// reaches the stream consumer.
    pub fn with_agent_event_tx(
        mut self,
        tx: tokio::sync::broadcast::Sender<crate::agent::AgentEvent>,
    ) -> Self {
        self.tool_context = self.tool_context.with_agent_event_tx(tx);
        self
    }

    /// Bind the run id used by per-tool-round checkpoints. Called per
    /// execution so a single `AgentLoop` (which is cheap to clone) can
    /// host successive runs.
    pub fn set_checkpoint_run(&mut self, run_id: impl Into<String>) {
        self.checkpoint_run_id = Some(run_id.into());
    }

    pub(crate) fn with_checkpoint_capability_binding(
        mut self,
        binding: crate::capability::RunCapabilityBindingV1,
    ) -> Self {
        self.checkpoint_capability_binding = Some(binding);
        self
    }

    pub(crate) fn checkpoint_capability_binding(
        &self,
    ) -> Option<&crate::capability::RunCapabilityBindingV1> {
        self.checkpoint_capability_binding.as_ref()
    }

    /// Return the immutable Hook executor captured by this loop's Run
    /// projection. Run-control requests must use this executor rather than a
    /// mutable Session-level reference so policy cannot change mid-run.
    pub(crate) fn hook_executor(&self) -> Option<Arc<dyn crate::hooks::HookExecutor>> {
        self.config.hook_engine.clone()
    }

    /// Permission policy, yolo lanes and round limit a fact run admits for this loop.
    pub(crate) fn fact_execution_policy(
        &self,
    ) -> (
        crate::permissions::PermissionPolicy,
        Vec<crate::queue::SessionLane>,
        usize,
    ) {
        let lanes = self
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.yolo_lanes.iter().copied().collect())
            .unwrap_or_default();
        (
            self.config.permission_policy.clone().unwrap_or_default(),
            lanes,
            self.config.max_tool_rounds,
        )
    }

    /// Continuation injection the legacy loop applied to an incomplete reply.
    /// `(enabled, max extra turns, tool-round ceiling)`.
    pub(crate) fn continuation_policy(&self) -> (bool, u32, usize) {
        (
            self.config.continuation_enabled,
            self.config.max_continuation_turns,
            self.config.max_tool_rounds,
        )
    }

    pub(crate) fn tool_executor_handle(&self) -> Arc<ToolExecutor> {
        Arc::clone(&self.tool_executor)
    }

    pub(crate) fn tool_context_handle(&self) -> ToolContext {
        self.tool_context.clone()
    }
}
