use thiserror::Error;

/// Validation failures for session-scoped reply / science review findings.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SessionReviewError {
    #[error("session review schema is unsupported")]
    UnsupportedSchema,
    #[error("session review field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("session review wire value exceeds its bounded encoding")]
    Encoding,
    #[error("session review transition from `{from}` to `{to}` is invalid")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("session review finding `{0}` was not found")]
    FindingNotFound(String),
    #[error("session review serialization failed: {0}")]
    Serialization(String),
}
