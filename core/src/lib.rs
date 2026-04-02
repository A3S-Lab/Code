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
pub mod agent_teams;
#[cfg(feature = "ahp")]
pub mod ahp;
pub mod commands;
pub(crate) mod composite_document_parser;
pub mod config;
pub mod context;
pub(crate) mod doc;
#[path = "document/consume.rs"]
pub(crate) mod document_consume;
#[path = "document/ocr.rs"]
pub mod document_ocr;
#[path = "document/parse_engine.rs"]
pub(crate) mod document_parse_engine;
#[path = "document/parser.rs"]
pub mod document_parser;
#[path = "document/parser_defaults.rs"]
pub(crate) mod document_parser_defaults;
#[path = "document/pipeline.rs"]
pub(crate) mod document_pipeline;
#[path = "document/pipeline_defaults.rs"]
pub(crate) mod document_pipeline_defaults;
#[path = "document/registry_factory.rs"]
pub(crate) mod document_registry_factory;
#[path = "document/render.rs"]
pub(crate) mod document_render;
#[path = "document/search_engine.rs"]
pub(crate) mod document_search_engine;
#[path = "document/search_scan.rs"]
pub(crate) mod document_search_scan;
#[path = "document/service_types.rs"]
pub(crate) mod document_service_types;
pub mod error;
pub mod file_history;
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
pub mod telemetry;
#[cfg(feature = "telemetry")]
pub mod telemetry_otel;
pub(crate) mod text;
pub mod tool_search;
pub mod tools;
pub mod undercover;

#[cfg(test)]
#[path = "document/engine_tests.rs"]
mod document_engine_tests;

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
pub use config::{
    CodeConfig, DocumentCacheConfig, DocumentOcrConfig, DocumentParserConfig, ModelConfig,
    ModelCost, ModelLimit, ModelModalities, ProviderConfig,
};
pub use error::{CodeError, Result};
pub use hooks::HookEngine;
pub use llm::{
    AnthropicClient, Attachment, ContentBlock, ImageSource, LlmClient, LlmResponse, Message,
    OpenAiClient, TokenUsage,
};
pub use orchestrator::AgentSlot;
pub use plugin::{Plugin, PluginContext, PluginManager, SkillPlugin};
pub use prompts::{
    SystemPromptSlots,
    // Verification agent
    AGENT_VERIFICATION,
    AGENT_VERIFICATION_RESTRICTIONS,
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
pub use tool_search::{ToolIndex, ToolMatch, ToolSearchConfig};
pub use tools::{ToolContext, ToolExecutor, ToolResult};
