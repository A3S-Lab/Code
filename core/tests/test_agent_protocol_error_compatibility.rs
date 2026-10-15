#![cfg(test)]

use a3s_code_core::{AgentProtocolHostError, CodeError};

const HOST_CODE: &str = AgentProtocolHostError::RunUnavailable.code();
const CORE_CODE: &str = CodeError::TaskSchedulerClosed.code();

// A v9.1.2 downstream client's exhaustive match must still compile. Adding a
// public HostError variant in a patch release would fail this boundary proof.
fn v912_client_category(error: &AgentProtocolHostError) -> &'static str {
    match error {
        AgentProtocolHostError::Protocol(_) => "protocol",
        AgentProtocolHostError::ReleaseMismatch => "release",
        AgentProtocolHostError::ReleaseProtocolMismatch => "release_protocol",
        AgentProtocolHostError::SessionMismatch => "session",
        AgentProtocolHostError::RunNotFound => "missing",
        AgentProtocolHostError::RunUnavailable => "unavailable",
        AgentProtocolHostError::SequenceOverflow => "sequence",
        AgentProtocolHostError::ChangeSetPending => "change_set_pending",
        AgentProtocolHostError::ChangeSetUnavailable => "change_set_unavailable",
        AgentProtocolHostError::Code(_) => "runtime",
    }
}

#[test]
fn published_v912_client_error_match_remains_exhaustive() {
    assert_eq!(
        v912_client_category(&AgentProtocolHostError::RunUnavailable),
        "unavailable"
    );
    assert_eq!(HOST_CODE, "a3s.code.agent_protocol.run_unavailable");
    assert_eq!(CORE_CODE, "TASK_SCHEDULER_CLOSED");
    assert_eq!(
        v912_core_category(&CodeError::TaskSchedulerClosed),
        "scheduler_closed"
    );
}

// Keep every published feature-gated variant in the downstream compile guard.
fn v912_core_category(error: &CodeError) -> &'static str {
    match error {
        CodeError::Config(_) => "config",
        CodeError::Llm(_) => "llm",
        CodeError::Tool { .. } => "tool",
        CodeError::Session(_) => "session",
        CodeError::SessionConfiguration { .. } => "session_configuration",
        CodeError::SessionInitialization { .. } => "session_initialization",
        CodeError::AsyncSessionBuildRequired { .. } => "async_build_required",
        CodeError::SessionClosed { .. } => "session_closed",
        CodeError::SessionBusy { .. } => "session_busy",
        CodeError::TaskAdmissionCancelled { .. } => "admission_cancelled",
        CodeError::TaskSchedulerClosed => "scheduler_closed",
        CodeError::TaskAdmissionLimit { .. } => "admission_limit",
        CodeError::RunIdentityConflict { .. } => "run_identity_conflict",
        CodeError::RunControl(_) => "run_control",
        CodeError::BudgetExhausted { .. } => "budget_exhausted",
        CodeError::Security(_) => "security",
        CodeError::Context(_) => "context",
        CodeError::Mcp(_) => "mcp",
        CodeError::Queue(_) => "queue",
        CodeError::Capability(_) => "capability",
        #[cfg(feature = "dynamic-workflow")]
        CodeError::Flow(_) => "flow",
        #[cfg(feature = "apofasi")]
        CodeError::TypedDecision(_) => "typed_decision",
        CodeError::Io(_) => "io",
        CodeError::Serialization(_) => "serialization",
        CodeError::Internal(_) => "internal",
    }
}
