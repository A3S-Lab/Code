//! Advanced SubAgent control plane.
//!
//! This module is an advanced control-plane API for explicit SubAgent lifecycle
//! management. Routine multi-agent composition should use the `task` /
//! `parallel_task` delegation tools.
//!
//! Broadcast-backed lifecycle APIs support:
//! - monitoring SubAgent behavior, planning, and execution
//! - dynamically controlling SubAgents: pause, resume, cancel, and inspect
//!
//! ## 架构
//!
//! ```text
//! AgentOrchestrator
//!     +-- spawn_subagent(SubAgentConfig)
//!     +-- AgentSession stream
//!     +-- broadcast OrchestratorEvent
//!     +-- pause/resume/cancel control signals
//! ```
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use a3s_code_core::orchestrator::{AgentOrchestrator, SubAgentConfig};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create a control plane backed by a real Agent.
//! let agent = std::sync::Arc::new(a3s_code_core::Agent::from_config(config).await?);
//! let orchestrator = AgentOrchestrator::from_agent(agent);
//!
//! // Subscribe to all events.
//! let mut events = orchestrator.subscribe_all();
//!
//! // Spawn a SubAgent.
//! let handle = orchestrator
//!     .spawn_subagent(
//!         SubAgentConfig::new("general", "Use glob to find Python files")
//!             .with_description("Analyze code")
//!             .with_max_steps(10),
//!     )
//!     .await?;
//!
//! // Monitor events.
//! tokio::spawn(async move {
//!     while let Ok(event) = events.recv().await {
//!         println!("Event: {:?}", event);
//!     }
//! });
//!
//! // Control the SubAgent lifecycle.
//! orchestrator.pause_subagent(&handle.id).await?;
//! orchestrator.resume_subagent(&handle.id).await?;
//!
//! # Ok(())
//! # }
//! ```

mod agent;
mod config;
mod control;
mod events;
mod handle;
mod state;
mod wrapper;

#[cfg(test)]
mod tests;

pub use agent::{AgentOrchestrator, SubAgentEventStream};
pub use config::{OrchestratorConfig, SubAgentActivity, SubAgentConfig, SubAgentInfo};
pub use control::ControlSignal;
pub use events::OrchestratorEvent;
pub use handle::SubAgentHandle;
pub use state::SubAgentState;
