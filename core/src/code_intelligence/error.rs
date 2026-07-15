use super::{CodePosition, LanguageId};
use crate::workspace::WorkspacePath;
use std::time::Duration;

/// Result type for semantic code intelligence operations.
pub type CodeIntelligenceResult<T> = std::result::Result<T, CodeIntelligenceError>;

/// Typed failure returned by a code intelligence runtime.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CodeIntelligenceError {
    #[error("Code Intelligence is unavailable: {message}")]
    Unavailable { message: String },

    #[error("Code Intelligence operation '{operation}' is unsupported: {message}")]
    Unsupported { operation: String, message: String },

    #[error("workspace path {path:?} cannot be used for Code Intelligence: {message}")]
    InvalidPath {
        path: WorkspacePath,
        message: String,
    },

    #[error("invalid position {position:?} for workspace document {path:?}")]
    InvalidPosition {
        path: WorkspacePath,
        position: CodePosition,
    },

    #[error("Code Intelligence operation was cancelled")]
    Cancelled,

    #[error("Code Intelligence operation '{operation}' timed out after {duration:?}")]
    Timeout {
        operation: String,
        duration: Duration,
    },

    #[error("Code Intelligence process for '{language}' exited: {message}")]
    ProcessExited {
        language: LanguageId,
        message: String,
    },

    #[error("Code Intelligence protocol error: {message}")]
    Protocol { message: String },
}

impl CodeIntelligenceError {
    /// Stable machine-readable code for API and tool adapters.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "CODE_INTELLIGENCE_UNAVAILABLE",
            Self::Unsupported { .. } => "CODE_INTELLIGENCE_UNSUPPORTED",
            Self::InvalidPath { .. } => "CODE_INTELLIGENCE_INVALID_PATH",
            Self::InvalidPosition { .. } => "CODE_INTELLIGENCE_INVALID_POSITION",
            Self::Cancelled => "CODE_INTELLIGENCE_CANCELLED",
            Self::Timeout { .. } => "CODE_INTELLIGENCE_TIMEOUT",
            Self::ProcessExited { .. } => "CODE_INTELLIGENCE_PROCESS_EXITED",
            Self::Protocol { .. } => "CODE_INTELLIGENCE_PROTOCOL_ERROR",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn errors_are_send_and_sync() {
        assert_send_sync::<CodeIntelligenceError>();
    }

    #[test]
    fn error_codes_are_stable_and_messages_keep_context() {
        let error = CodeIntelligenceError::Timeout {
            operation: "document_symbols".to_string(),
            duration: Duration::from_secs(2),
        };

        assert_eq!(error.code(), "CODE_INTELLIGENCE_TIMEOUT");
        assert!(error.to_string().contains("document_symbols"));
        assert!(error.to_string().contains("2s"));
    }
}
