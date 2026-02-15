//! A3S Code Core Library
//!
//! Embeddable AI agent library with tool execution capabilities.
//! This crate contains all business logic extracted from the A3S Code agent,
//! enabling direct Rust API usage without a gRPC server.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, AgentEvent};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let agent = Agent::builder()
//!     .model("claude-sonnet-4-20250514")
//!     .api_key("sk-ant-...")
//!     .workspace("/my-project")
//!     .build()
//!     .await?;
//!
//! // Non-streaming
//! let result = agent.send("What files handle auth?").await?;
//! println!("{}", result.text);
//!
//! // Streaming
//! let mut rx = agent.stream("Refactor auth").await?;
//! while let Some(event) = rx.recv().await {
//!     match event {
//!         AgentEvent::TextDelta { text } => print!("{text}"),
//!         AgentEvent::End { .. } => break,
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! Agent (facade)
//!   +-- AgentLoop (core execution engine)
//!   |     +-- LlmClient (Anthropic / OpenAI)
//!   |     +-- ToolExecutor (11 builtin tools + dynamic skills)
//!   |     +-- HITL Confirmation
//!   +-- SessionManager (multi-session support)
//!   +-- ToolMetrics / Cost Tracking
//! ```

pub mod agent;
pub mod agent_api;
pub mod checkpoint;
pub mod config;
pub mod context;
pub mod context_store;
pub mod export;
pub mod file_history;
pub mod hitl;
pub mod hooks;
pub mod lane_integration;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod planning;
pub mod project_memory;
pub mod prompts;
pub mod queue;
pub mod reflection;
pub mod retry;
pub mod security;
pub mod session;
pub mod session_lane_queue;
pub mod store;
pub mod subagent;
pub mod telemetry;
pub mod todo;
pub mod tools;

// Re-export key types at crate root for ergonomic usage
pub use agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
pub use agent_api::Agent;
pub use config::{CodeConfig, ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
pub use hooks::HookEngine;
pub use llm::{AnthropicClient, LlmClient, LlmResponse, Message, OpenAiClient, TokenUsage};
pub use session::{SessionConfig, SessionManager, SessionState};
pub use tools::{ToolContext, ToolExecutor, ToolResult};
