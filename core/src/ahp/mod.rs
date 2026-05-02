//! AHP (Agent Harness Protocol) Integration
//!
//! Provides external supervision and governance for A3S Code agents via the
//! Agent Harness Protocol v2.3. This module bridges A3S Code's hook system
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
//!     AhpTransport::Http {
//!         url: "http://localhost:8080/ahp".to_string(),
//!         auth: None,
//!     },
//!     10_000  // 10 second idle threshold
//! ).await?;
//!
//! // Create agent with AHP supervision
//! let agent = Agent::new("agent.acl").await?;
//! let session = agent.session(
//!     "/workspace",
//!     Some(SessionOptions::default().with_hook_executor(std::sync::Arc::new(ahp)))
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
//! | `AgentEvent::Start` | `RunLifecycle(executing)` | No |
//! | `AgentEvent::PlanningStart` | `RunLifecycle(planning)` | No |
//! | `AgentEvent::TaskUpdated` | `TaskList` | No |
//! | `AgentEvent::End` | `RunLifecycle(completed)` + `Verification` | No |
//! | `AgentEvent::Error` | `RunLifecycle(failed)` | No |
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
mod contract;
#[cfg(feature = "ahp")]
mod executor;

#[cfg(feature = "ahp")]
pub use contract::{agent_event_to_ahp_events, cancelled_run_event, tasks_to_ahp_items};
#[cfg(feature = "ahp")]
pub use executor::AhpHookExecutor;

#[cfg(feature = "ahp")]
pub use a3s_ahp::{
    AhpClient, AhpError, AhpEvent, AhpNotification, AhpRequest, AhpResponse, ArtifactRef,
    AuthConfig, ConfirmationDecision, ContextPerceptionDecision, ContextPerceptionEvent, Decision,
    EventContext, EventType, EvidenceRef, Fact, HeartbeatEvent, IdleDecision, IdleEvent,
    InjectedContext, IntentDetectionDecision, IntentDetectionEvent, MemoryRecallDecision,
    MemoryRecallEvent, MemorySummary, PerceptionConstraints, PerceptionContext, PerceptionDomain,
    PerceptionFreshness, PerceptionIntent, PerceptionModality, PerceptionTarget, PerceptionUrgency,
    PlanningDecision, PlanningEvent, QueryRequest, QueryResponse, RateLimitDecision,
    RateLimitEvent, ReasoningDecision, ReasoningEvent, RunLifecycleEvent, RunStatus, SessionStats,
    SuccessEvent, TargetHints, TaskItem, TaskListEvent, TaskStatus, Transport as AhpTransport,
    VerificationCheck, VerificationEvent, VerificationStatus, PROTOCOL_VERSION,
};

// Re-export types from protocol that are not directly in a3s_ahp root
#[cfg(feature = "ahp")]
pub use a3s_ahp::protocol::{
    ConfirmationEvent, ConfirmationType, PlanningStrategy, RateLimitType, ReasoningType,
};

#[cfg(not(feature = "ahp"))]
compile_error!("AHP feature is not enabled. Add `ahp` feature to use this module.");
