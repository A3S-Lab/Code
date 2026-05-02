//! A3S Code Core Library
//!
//! Harness-driven runtime for coding agents.
//!
//! `Agent` and `AgentSession` are the primary 2.0 API. Lower-level session
//! runtime state is internal; persistence data flows through `store::SessionData`.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, AgentEvent};
//!
//! # async fn run() -> anyhow::Result<()> {
//! // From an ACL-compatible config file path (.acl)
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
//! Agent (config-driven facade)
//!   +-- AgentSession (workspace-bound execution API)
//!       +-- internal turn runner
//!       +-- ContextAssembler / ContextProvider
//!       +-- ToolSelector
//!       +-- ToolExecutor
//!       +-- ProgramExecutor (PTC)
//!       +-- SkillRegistry
//!       +-- Permission / confirmation
//!       +-- Trace / artifacts / verification evidence
//!
//! Advanced infrastructure:
//!   +-- optional lane queues for explicit external/hybrid dispatch
//!   +-- Orchestrator for direct SubAgent lifecycle control
//! ```

pub(crate) mod agent;
pub(crate) mod agent_api;
#[cfg(feature = "ahp")]
pub mod ahp;
pub mod commands;
pub(crate) mod compaction;
pub mod config;
pub mod context;
pub mod error;
pub(crate) mod file_history;
pub(crate) mod git;
pub mod hitl;
pub mod hooks;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod orchestrator;
pub mod permissions;
pub mod planning;
pub mod plugin;
pub mod program;
pub(crate) mod prompts;
pub mod queue;
pub(crate) mod retry;
pub mod sandbox;
pub mod security;
pub(crate) mod session_lane_queue;
pub mod skills;
pub mod store;
pub mod subagent;
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub mod telemetry_otel;
pub(crate) mod text;
pub mod tools;
pub mod trace;
pub mod verification;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentEvent, AgentResult};
pub use agent_api::{Agent, AgentSession, BtwResult, SessionOptions, ToolCallResult};
pub use config::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
pub use error::{CodeError, Result};
pub use llm::{
    clear_http_metrics_callback, set_http_metrics_callback, AnthropicClient, Attachment,
    ContentBlock, HttpMetricsCallback, HttpMetricsRecord, ImageSource, LlmClient, LlmResponse,
    Message, OpenAiClient, TokenUsage,
};
pub use prompts::{AgentStyle, DetectionConfidence, PlanningMode, SystemPromptSlots};
