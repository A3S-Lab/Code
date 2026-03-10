//! AHP (Agent Harness Protocol) Integration
//!
//! Provides external supervision and governance for A3S Code agents via the
//! Agent Harness Protocol v2.0. This module bridges A3S Code's hook system
//! with AHP's event-driven supervision model.
//!
//! ## Overview
//!
//! AHP enables external harness servers to:
//! - Intercept and validate agent actions before execution
//! - Monitor agent behavior and outputs
//! - Enforce depth-aware policies for nested agents
//! - Query agents for guidance on ambiguous operations
//! - Batch process multiple events for efficiency
//!
//! ## Architecture
//!
//! ```text
//! A3S Code Agent
//!   └── HookEngine
//!         └── AhpHookExecutor (implements HookExecutor)
//!               └── AhpClient
//!                     └── Transport (stdio / HTTP / WebSocket)
//!                           └── External Harness Server
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use a3s_code_core::{Agent, SessionOptions};
//! use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create AHP executor with HTTP transport
//! let ahp = AhpHookExecutor::new(
//!     AhpTransport::http("http://localhost:8080/ahp", None)
//! ).await?;
//!
//! // Create agent with AHP supervision
//! let agent = Agent::new("agent.hcl").await?;
//! let session = agent.session(
//!     "/workspace",
//!     Some(SessionOptions::default().with_ahp_executor(ahp))
//! )?;
//!
//! // All agent actions are now supervised by the harness
//! session.send("Refactor auth module", None).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Event Mapping
//!
//! A3S Code hooks are mapped to AHP events:
//!
//! | A3S Code Hook | AHP Event | Blocking |
//! |---------------|-----------|----------|
//! | `PreToolUse` | `PreAction` | Yes |
//! | `PostToolUse` | `PostAction` | No |
//! | `PrePrompt` | `PrePrompt` | Yes |
//! | `GenerateStart` | `PrePrompt` | Yes |
//! | `PostResponse` | `PostAction` | No |
//! | `SessionStart` | `SessionStart` | No |
//! | `SessionEnd` | `SessionEnd` | No |
//! | `OnError` | `Error` | No |
//!
//! ## Depth-Aware Policies
//!
//! AHP supports depth tracking for nested agents:
//! - Depth 0: User-initiated agent
//! - Depth 1: First-level sub-agent
//! - Depth 2+: Deeply nested agents
//!
//! Harness servers can enforce stricter policies at higher depths.

#[cfg(feature = "ahp")]
mod executor;

#[cfg(feature = "ahp")]
pub use executor::AhpHookExecutor;

#[cfg(feature = "ahp")]
pub use a3s_ahp::{
    AhpClient, AhpError, AhpEvent, AuthConfig, Decision, EventType, Transport as AhpTransport,
};

#[cfg(not(feature = "ahp"))]
compile_error!("AHP feature is not enabled. Add `ahp` feature to use this module.");
