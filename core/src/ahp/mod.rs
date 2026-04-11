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
//! - **Detect idle state** for background consolidation (dream system)
//! - **Context-aware decisions** with rich session context
//!
//! ## Architecture
//!
//! ```text
//! A3S Code Agent
//!   └── HookEngine
//!         └── AhpHookExecutor (implements HookExecutor)
//!               ├── Idle Tracker (fires Idle events when agent is idle)
//!               ├── EventContext Builder (enriches events with memory/facts)
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
//! // Create AHP executor with HTTP transport and 10s idle threshold
//! let ahp = AhpHookExecutor::new_with_config(
//!     AhpTransport::http("http://localhost:8080/ahp", None),
//!     10_000  // 10 second idle threshold
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
//! ## Idle Detection (Dream System)
//!
//! When idle detection is enabled, AHP fires `Idle` events when the agent
//! has been inactive for a configurable threshold duration. This enables:
//! - Background memory consolidation
//! - Cross-session fact processing
//! - Periodic cleanup and optimization
//!
//! The harness server can respond with `IdleDecision::Allow` to permit
//! background consolidation or `IdleDecision::Defer` to postpone it.
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
    AhpClient, AhpError, AhpEvent, AhpNotification, AhpRequest, AhpResponse, AuthConfig,
    ConfirmationDecision, ContextPerceptionDecision, ContextPerceptionEvent, Decision,
    EventContext, EventType, Fact, HeartbeatEvent, IdleDecision, IdleEvent, InjectedContext,
    IntentDetectionDecision, IntentDetectionEvent, MemoryRecallDecision, MemoryRecallEvent,
    MemorySummary, PerceptionConstraints, PerceptionContext, PerceptionDomain, PerceptionFreshness,
    PerceptionIntent, PerceptionModality, PerceptionTarget, PerceptionUrgency, PlanningDecision,
    PlanningEvent, QueryRequest, QueryResponse, RateLimitDecision, RateLimitEvent,
    ReasoningDecision, ReasoningEvent, SessionStats, SuccessEvent, TargetHints,
    Transport as AhpTransport,
};

// Re-export types from protocol that are not directly in a3s_ahp root
#[cfg(feature = "ahp")]
pub use a3s_ahp::protocol::{
    ConfirmationEvent, ConfirmationType, PlanningStrategy, RateLimitType, ReasoningType,
};

#[cfg(not(feature = "ahp"))]
compile_error!("AHP feature is not enabled. Add `ahp` feature to use this module.");
