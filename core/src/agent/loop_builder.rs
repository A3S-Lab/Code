use super::{AgentConfig, AgentLoop};
use crate::llm::{LlmClient, ModelGenerationAdmission};
use crate::loop_checkpoint::LoopCheckpointSink;
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::{ToolContext, ToolExecutor};
use std::sync::Arc;

impl AgentLoop {
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
            tool_executor,
            tool_context,
            config,
            command_queue: None,
            checkpoint_sink: None,
            checkpoint_run_id: None,
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
        self
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
}
