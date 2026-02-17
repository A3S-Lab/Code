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

// ============================================================================
// Lock Poisoning Helpers (Phase 3b)
// ============================================================================

/// Acquire a read guard, recovering from poison if the lock was poisoned.
///
/// Non-security code should never panic on a poisoned lock. The data may
/// be in an inconsistent state, but crashing the entire process is worse
/// than serving stale data in a coding agent context.
pub(crate) fn read_or_recover<T>(
    lock: &std::sync::RwLock<T>,
) -> std::sync::RwLockReadGuard<'_, T> {
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
