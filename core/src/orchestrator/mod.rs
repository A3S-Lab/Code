//! Agent Orchestrator - 主子智能体协调器
//!
//! 基于 a3s-event 实现的统一事件总线，支持：
//! - 实时监控所有子智能体的行为、规划和执行
//! - 主智能体动态控制子智能体（暂停、恢复、取消、参数调整）
//! - 可插拔的事件 provider（默认内存，可选 NATS）
//!
//! ## 架构
//!
//! ```text
//! AgentOrchestrator (主智能体)
//!     ↓ 订阅事件
//! ┌───────────────────────────────┐
//! │   a3s-event EventBus          │
//! │ (Memory / NATS / Custom)      │
//! └───────────────────────────────┘
//!     ↑ 发布事件    ↓ 控制信号
//! SubAgentWrapper (子智能体)
//! ```
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use a3s_code_core::orchestrator::{AgentOrchestrator, SubAgentConfig};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // 创建 orchestrator (默认使用内存 provider)
//! let orchestrator = AgentOrchestrator::new_memory();
//!
//! // 订阅所有事件
//! let mut events = orchestrator.subscribe_all().await?;
//!
//! // 启动子智能体
//! let handle = orchestrator.spawn_subagent(SubAgentConfig {
//!     agent_type: "general".to_string(),
//!     description: "Analyze code".to_string(),
//!     prompt: "Use glob to find Python files".to_string(),
//!     permissive: true,
//!     max_steps: Some(10),
//! }).await?;
//!
//! // 监控事件
//! tokio::spawn(async move {
//!     while let Some(event) = events.recv().await {
//!         println!("Event: {:?}", event);
//!     }
//! });
//!
//! // 控制子智能体
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

pub use crate::agent_teams::TeamRole;
pub use agent::{AgentOrchestrator, SubAgentEventStream};
pub use config::{AgentSlot, OrchestratorConfig, SubAgentActivity, SubAgentConfig, SubAgentInfo};
pub use control::ControlSignal;
pub use events::{OrchestratorEvent, SubAgentEventPayload};
pub use handle::SubAgentHandle;
pub use state::SubAgentState;
pub use wrapper::SubAgentWrapper;
