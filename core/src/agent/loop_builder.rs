use super::{AgentConfig, AgentLoop, ModelMiddlewareHealthSnapshot, ModelMiddlewareObs};
use crate::llm::{LlmClient, ModelGenerationAdmission};
use crate::loop_checkpoint::LoopCheckpointSink;
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::{ToolContext, ToolExecutor};
use std::sync::Arc;

impl AgentLoop {
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

    pub(crate) fn sanitize_tool_output(&self, text: &str) -> String {
        match self.config.security_provider.as_ref() {
            Some(provider) => provider.sanitize_output(text),
            None => text.to_string(),
        }
    }

    /// Character budget for the fact-log compactor.
    ///
    /// Auto-compact stays off this path until the host enables it. The budget is
    /// the configured token window times the threshold, at about four characters
    /// per token, so a filled turn compacts before the next model call.
    pub(crate) fn fact_compact_after_chars(&self) -> usize {
        if !self.config.auto_compact {
            return 1_000_000;
        }
        let tokens = (self.config.max_context_tokens as f32 * self.config.auto_compact_threshold)
            .max(1.0) as usize;
        tokens.saturating_mul(4).max(1)
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
        tool_context: ToolContext,
        config: AgentConfig,
    ) -> Self {
        let model_generation_admission =
            ModelGenerationAdmission::new(llm_client.model_generation_concurrency());
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
        }
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

    pub(crate) fn tool_executor_handle(&self) -> Arc<ToolExecutor> {
        Arc::clone(&self.tool_executor)
    }

    pub(crate) fn tool_context_handle(&self) -> ToolContext {
        self.tool_context.clone()
    }
}
