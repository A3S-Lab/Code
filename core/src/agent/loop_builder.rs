use super::{AgentConfig, AgentLoop};
use crate::llm::LlmClient;
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
        Self {
            llm_client,
            tool_executor,
            tool_context,
            config,
            command_queue: None,
            checkpoint_sink: None,
            checkpoint_run_id: None,
        }
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
