//! Typed error enum for A3S Code Core
//!
//! Provides categorized errors that SDK consumers can match on programmatically,
//! instead of receiving opaque `anyhow::Error` strings.
//!
//! ## Migration Strategy
//!
//! The `Internal` variant wraps `anyhow::Error` via `#[from]`, allowing
//! gradual migration: call sites that haven't been updated yet auto-convert
//! through `?`. Over time, each call site replaces `anyhow::anyhow!(...)`
//! with a specific variant like `CodeError::Config(...)`.

use thiserror::Error;

/// Async resource whose initialization is part of building a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBuildResource {
    MemoryStore,
    SessionStore,
    Queue,
    Mcp,
    RlTrajectory,
}

impl std::fmt::Display for SessionBuildResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MemoryStore => "memory store",
            Self::SessionStore => "session store",
            Self::Queue => "session queue",
            Self::Mcp => "MCP",
            Self::RlTrajectory => "RL trajectory recorder",
        })
    }
}

/// Crate-wide result type alias.
pub type Result<T> = std::result::Result<T, CodeError>;

/// Categorized error type for A3S Code Core.
///
/// SDK bindings (Python/Node) can match on the variant to expose typed
/// exceptions (e.g., `CodeConfigError`, `CodeLlmError`).
#[derive(Debug, Error)]
pub enum CodeError {
    /// Configuration loading or parsing error
    #[error("Config error: {0}")]
    Config(String),

    /// LLM provider communication error
    #[error("LLM error: {0}")]
    Llm(String),

    /// Tool execution error
    #[error("Tool error: {tool}: {message}")]
    Tool { tool: String, message: String },

    /// Session management error
    #[error("Session error: {0}")]
    Session(String),

    /// A session option is missing, malformed, or conflicts with another option.
    #[error("Invalid session configuration for '{field}': {message}")]
    SessionConfiguration {
        field: &'static str,
        message: String,
    },

    /// A session resource could not be initialized.
    #[error("Failed to initialize {resource}: {message}")]
    SessionInitialization {
        resource: SessionBuildResource,
        message: String,
    },

    /// The synchronous compatibility factory was asked to initialize an
    /// async-only resource. Call `Agent::session_builder(...).build().await`.
    #[error(
        "{resource} requires asynchronous session construction; use Agent::session_builder(...).build().await"
    )]
    AsyncSessionBuildRequired { resource: SessionBuildResource },

    /// Session has been closed; further operations are rejected.
    ///
    /// Returned by `send`/`stream` (and their variants) after
    /// [`AgentSession::close`](crate::agent_api::AgentSession::close)
    /// — or [`Agent::close`](crate::agent_api::Agent::close) — has been called.
    #[error("Session '{session_id}' is closed")]
    SessionClosed { session_id: String },

    /// Another conversation operation is already active on this session.
    ///
    /// Sessions serialize conversation state, so callers must wait for the
    /// active operation's returned future or stream handle to finish before
    /// starting another one.
    #[error("Session '{session_id}' already has an active operation")]
    SessionBusy { session_id: String },

    /// Global task admission was cancelled before an execution slot opened.
    #[error("Task admission for session '{session_id}' was cancelled")]
    TaskAdmissionCancelled { session_id: String },

    /// The owning agent's global task scheduler no longer accepts work.
    #[error("Task scheduler is closed")]
    TaskSchedulerClosed,

    /// A host replayed a run id with different immutable session or input
    /// identity. The existing run is preserved and no work is started.
    #[error("Run '{run_id}' is already bound to different immutable input")]
    RunIdentityConflict { run_id: String },

    /// A host-supplied [`BudgetGuard`](crate::budget::BudgetGuard) denied
    /// the operation. The session is not closed — callers can re-try
    /// after the host has re-allocated budget.
    #[error("Budget exhausted on '{resource}': {reason}")]
    BudgetExhausted { resource: String, reason: String },

    /// Security subsystem error
    #[error("Security error: {0}")]
    Security(String),

    /// Context provider or context store error
    #[error("Context error: {0}")]
    Context(String),

    /// MCP (Model Context Protocol) error
    #[error("MCP error: {0}")]
    Mcp(String),

    /// Queue or lane error
    #[error("Queue error: {0}")]
    Queue(String),

    /// I/O error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Catch-all for errors not yet migrated to a specific variant.
    ///
    /// The `#[from] anyhow::Error` conversion enables gradual migration:
    /// any function returning `anyhow::Result` can be called with `?` from
    /// a function returning `crate::error::Result` without changes.
    #[error("{0:#}")]
    Internal(#[from] anyhow::Error),
}

impl CodeError {
    /// Stable machine-readable code for SDK and service boundaries.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "CONFIG_ERROR",
            Self::Llm(_) => "LLM_ERROR",
            Self::Tool { .. } => "TOOL_ERROR",
            Self::Session(_) => "SESSION_ERROR",
            Self::SessionConfiguration { .. } => "SESSION_CONFIGURATION_ERROR",
            Self::SessionInitialization { .. } => "SESSION_INITIALIZATION_ERROR",
            Self::AsyncSessionBuildRequired { .. } => "ASYNC_SESSION_BUILD_REQUIRED",
            Self::SessionClosed { .. } => "SESSION_CLOSED",
            Self::SessionBusy { .. } => "SESSION_BUSY",
            Self::TaskAdmissionCancelled { .. } => "TASK_ADMISSION_CANCELLED",
            Self::TaskSchedulerClosed => "TASK_SCHEDULER_CLOSED",
            Self::RunIdentityConflict { .. } => "RUN_IDENTITY_CONFLICT",
            Self::BudgetExhausted { .. } => "BUDGET_EXHAUSTED",
            Self::Security(_) => "SECURITY_ERROR",
            Self::Context(_) => "CONTEXT_ERROR",
            Self::Mcp(_) => "MCP_ERROR",
            Self::Queue(_) => "QUEUE_ERROR",
            Self::Io(_) => "IO_ERROR",
            Self::Serialization(_) => "SERIALIZATION_ERROR",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

// ============================================================================
// Lock Poisoning Helpers (Phase 3b)
// ============================================================================

/// Acquire a read guard, recovering from poison if the lock was poisoned.
///
/// Non-security code should never panic on a poisoned lock. The data may
/// be in an inconsistent state, but crashing the entire process is worse
/// than serving stale data in a coding agent context.
pub(crate) fn read_or_recover<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|p| p.into_inner())
}

/// Acquire a write guard, recovering from poison if the lock was poisoned.
///
/// See [`read_or_recover`] for rationale.
pub(crate) fn write_or_recover<T>(
    lock: &std::sync::RwLock<T>,
) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_error_config() {
        let err = CodeError::Config("missing API key".to_string());
        assert!(err.to_string().contains("Config error"));
        assert!(err.to_string().contains("missing API key"));
    }

    #[test]
    fn test_code_error_llm() {
        let err = CodeError::Llm("rate limited".to_string());
        assert!(err.to_string().contains("LLM error"));
    }

    #[test]
    fn test_code_error_tool() {
        let err = CodeError::Tool {
            tool: "bash".to_string(),
            message: "command not found".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("bash"));
        assert!(msg.contains("command not found"));
    }

    #[test]
    fn test_code_error_session() {
        let err = CodeError::Session("not found".to_string());
        assert!(err.to_string().contains("Session error"));
    }

    #[test]
    fn test_code_error_session_configuration_keeps_field_identity() {
        let err = CodeError::SessionConfiguration {
            field: "session_id",
            message: "must not be empty".to_string(),
        };
        assert!(err.to_string().contains("session_id"));
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_code_error_session_busy() {
        let err = CodeError::SessionBusy {
            session_id: "session-1".to_string(),
        };
        assert!(err.to_string().contains("session-1"));
        assert!(err.to_string().contains("active operation"));
    }

    #[test]
    fn test_code_error_security() {
        let err = CodeError::Security("taint detected".to_string());
        assert!(err.to_string().contains("Security error"));
    }

    #[test]
    fn test_code_error_context() {
        let err = CodeError::Context("provider failed".to_string());
        assert!(err.to_string().contains("Context error"));
    }

    #[test]
    fn test_code_error_mcp() {
        let err = CodeError::Mcp("connection refused".to_string());
        assert!(err.to_string().contains("MCP error"));
    }

    #[test]
    fn test_code_error_queue() {
        let err = CodeError::Queue("lane full".to_string());
        assert!(err.to_string().contains("Queue error"));
    }

    #[test]
    fn test_code_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: CodeError = io_err.into();
        assert!(matches!(err, CodeError::Io(_)));
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn test_code_error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: CodeError = json_err.into();
        assert!(matches!(err, CodeError::Serialization(_)));
    }

    #[test]
    fn test_code_error_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: CodeError = anyhow_err.into();
        assert!(matches!(err, CodeError::Internal(_)));
        assert!(err.to_string().contains("something went wrong"));
    }

    #[test]
    fn stable_error_codes_cover_control_flow_variants() {
        assert_eq!(
            CodeError::SessionBusy {
                session_id: "session-1".to_string(),
            }
            .code(),
            "SESSION_BUSY"
        );
        assert_eq!(
            CodeError::SessionClosed {
                session_id: "session-1".to_string(),
            }
            .code(),
            "SESSION_CLOSED"
        );
        assert_eq!(
            CodeError::RunIdentityConflict {
                run_id: "run-1".to_string(),
            }
            .code(),
            "RUN_IDENTITY_CONFLICT"
        );
        assert_eq!(
            CodeError::BudgetExhausted {
                resource: "tokens".to_string(),
                reason: "limit".to_string(),
            }
            .code(),
            "BUDGET_EXHAUSTED"
        );
        assert_eq!(
            CodeError::TaskAdmissionCancelled {
                session_id: "session-1".to_string(),
            }
            .code(),
            "TASK_ADMISSION_CANCELLED"
        );
        assert_eq!(
            CodeError::TaskSchedulerClosed.code(),
            "TASK_SCHEDULER_CLOSED"
        );
    }

    #[test]
    fn test_code_error_question_mark_from_anyhow() {
        fn inner() -> anyhow::Result<()> {
            anyhow::bail!("inner error")
        }

        fn outer() -> Result<()> {
            inner()?; // anyhow::Error -> CodeError::Internal via #[from]
            Ok(())
        }

        let result = outer();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CodeError::Internal(_)));
    }

    #[test]
    fn test_read_or_recover_normal() {
        let lock = std::sync::RwLock::new(42);
        let guard = read_or_recover(&lock);
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_write_or_recover_normal() {
        let lock = std::sync::RwLock::new(42);
        let mut guard = write_or_recover(&lock);
        *guard = 99;
        drop(guard);
        assert_eq!(*read_or_recover(&lock), 99);
    }

    #[test]
    fn test_read_or_recover_poisoned() {
        let lock = std::sync::RwLock::new(42);
        // Poison the lock by panicking while holding a write guard
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock.write().unwrap();
            panic!("intentional poison");
        }));
        // Should recover without panicking
        let guard = read_or_recover(&lock);
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_write_or_recover_poisoned() {
        let lock = std::sync::RwLock::new(42);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock.write().unwrap();
            panic!("intentional poison");
        }));
        let mut guard = write_or_recover(&lock);
        *guard = 100;
        assert_eq!(*guard, 100);
    }
}
