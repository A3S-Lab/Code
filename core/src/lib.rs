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
//! // From a config file path (.hcl or .json)
//! let agent = Agent::new("agent.hcl").await?;
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
//!   +-- CodeConfig (HCL / JSON)
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
pub mod config;
pub mod context;
pub mod error;
pub mod file_history;
pub mod hitl;
pub mod hooks;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod planning;
pub(crate) mod prompts;
pub mod queue;
pub(crate) mod retry;
pub mod sandbox;
pub mod security;
pub mod session;
pub mod session_lane_queue;
pub mod skills;
pub mod store;
pub(crate) mod subagent;
pub mod telemetry;
pub mod tools;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
pub use agent_api::{Agent, AgentSession, SessionOptions, ToolCallResult};
pub use config::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
pub use error::{CodeError, Result};
pub use hooks::HookEngine;
pub use llm::{
    AnthropicClient, Attachment, ContentBlock, ImageSource, LlmClient, LlmResponse, Message,
    OpenAiClient, TokenUsage,
};
pub use queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionLane, SessionQueueConfig,
    SessionQueueStats, TaskHandlerMode,
};
pub use sandbox::SandboxConfig;
pub use session::{SessionConfig, SessionManager, SessionState};
pub use session_lane_queue::SessionLaneQueue;
pub use skills::{builtin_skills, Skill, SkillKind};
pub use tools::{ToolContext, ToolExecutor, ToolResult};
