//! A3S Code Core Library
//!
//! Embeddable AI agent library with tool execution capabilities.
//! This crate contains all business logic extracted from the A3S Code agent,
//! enabling direct Rust API usage as an embedded library.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, AgentEvent};
//!
//! # async fn run() -> anyhow::Result<()> {
//! // From an ACL-compatible config file path (.acl, or legacy .hcl)
//! let agent = Agent::new("agent.acl").await?;
//!
//! // Create a workspace-bound session
//! let session = agent.session("/my-project", None)?;
//!
//! // Non-streaming
//! let result = session.send("What files handle auth?", None).await?;
//! println!("{}", result.text);
//!
//! // Streaming (AgentEvent is #[non_exhaustive])
//! let (mut rx, _handle) = session.stream("Refactor auth", None).await?;
//! while let Some(event) = rx.recv().await {
//!     match event {
//!         AgentEvent::TextDelta { text } => print!("{text}"),
//!         AgentEvent::End { .. } => break,
//!         _ => {} // required: #[non_exhaustive]
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! Agent (facade — config-driven, workspace-independent)
//!   +-- LlmClient (Anthropic / OpenAI)
//!   +-- CodeConfig (ACL-compatible config; legacy .hcl filenames accepted)
//!   +-- SessionManager (multi-session support)
//!         |
//!         +-- AgentSession (workspace-bound)
//!               +-- AgentLoop (core execution engine)
//!               |     +-- ToolExecutor (14 tools: 11 builtin + 3 skill discovery)
//!               |     +-- LlmPlanner (JSON-structured planning)
//!               |     +-- HITL Confirmation
//!               +-- HookEngine (8 lifecycle events)
//!               +-- Security (sanitizer, taint, injection detection, audit)
//!               +-- Memory (episodic, semantic, procedural, working)
//!               +-- MCP (JSON-RPC 2.0, stdio + HTTP+SSE)
//!               +-- Cost Tracking / Telemetry
//! ```

pub mod agent;
pub mod agent_api;
pub mod agent_teams;
#[cfg(feature = "ahp")]
pub mod ahp;
pub mod commands;
pub mod config;
pub mod context;
pub mod error;
pub mod file_history;
pub mod git;
pub mod hitl;
pub mod hooks;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod orchestrator;
pub mod permissions;
pub mod planning;
pub mod plugin;
pub(crate) mod prompts;
pub mod queue;
pub(crate) mod retry;
pub mod sandbox;
pub mod scheduler;
pub mod security;
pub mod session;
pub mod session_lane_queue;
pub mod skills;
pub mod store;
pub(crate) mod subagent;
pub mod task;
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub mod telemetry_otel;
pub(crate) mod text;
pub mod tool_search;
pub mod tools;
pub mod undercover;

// Re-export key types at crate root for ergonomic usage
pub use a3s_lane::MetricsSnapshot;
pub use agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
pub use agent_api::{Agent, AgentSession, BtwResult, SessionOptions, ToolCallResult};
pub use agent_teams::{
    AgentExecutor, AgentTeam, TeamConfig, TeamMember, TeamMemberOptions, TeamMessage, TeamRole,
    TeamRunResult, TeamRunner, TeamTaskBoard,
};
pub use commands::{
    CommandAction, CommandContext, CommandOutput, CommandRegistry, CronCancelCommand,
    CronListCommand, LoopCommand, SlashCommand,
};
pub use config::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
pub use error::{CodeError, Result};
pub use hooks::HookEngine;
pub use llm::{
    clear_http_metrics_callback, set_http_metrics_callback, AnthropicClient, Attachment,
    ContentBlock, HttpMetricsCallback, HttpMetricsRecord, ImageSource, LlmClient, LlmResponse,
    Message, OpenAiClient, TokenUsage,
};
pub use orchestrator::AgentSlot;
pub use plugin::{Plugin, PluginContext, PluginManager, SkillPlugin};
pub use prompts::{
    AgentStyle,
    DetectionConfidence,
    PlanningMode,
    SystemPromptSlots,
    // Verification agent
    AGENT_VERIFICATION,
    AGENT_VERIFICATION_RESTRICTIONS,
    // Intent classification
    INTENT_CLASSIFY_SYSTEM,
    // Prompt suggestion
    PROMPT_SUGGESTION,
    // Session memory
    SESSION_MEMORY_TEMPLATE,
    // Existing prompts
    SUBAGENT_EXPLORE,
    SUBAGENT_PLAN,
    SUBAGENT_SUMMARY,
    SUBAGENT_TITLE,
    // Undercover mode
    UNDERCOVER_INSTRUCTIONS,
};
pub use queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionCommand, SessionLane,
    SessionQueueConfig, SessionQueueStats, TaskHandlerMode,
};
pub use sandbox::SandboxConfig;
pub use scheduler::{CronScheduler, ScheduledFire, ScheduledTask, ScheduledTaskInfo};
pub use session::{SessionConfig, SessionManager, SessionState};
pub use session_lane_queue::SessionLaneQueue;
pub use skills::{builtin_skills, Skill, SkillKind};
pub use subagent::{load_agents_from_dir, AgentDefinition, AgentRegistry};
pub use task::manager::TaskEvent;
pub use task::{
    AgentProgress, Coordinator, IdlePhase, IdleTask, IdleTurn, ProgressTracker, Task, TaskId,
    TaskManager, TaskResult, TaskStatus, TaskTokenUsage, TaskType, ToolActivity,
};
pub use tool_search::{ToolIndex, ToolMatch, ToolSearchConfig};
pub use tools::{ToolContext, ToolExecutor, ToolResult};
