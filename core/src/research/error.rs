use thiserror::Error;

/// Validation failures for native scientific research contracts.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ResearchContractError {
    #[error("research contract schema is unsupported")]
    UnsupportedSchema,
    #[error("research contract field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("research contract field `{0}` is not a canonical SHA-256 digest")]
    InvalidDigest(&'static str),
    #[error("research contract digest for `{0}` does not match its contents")]
    DigestMismatch(&'static str),
    #[error("research contract transition from `{from}` to `{to}` is invalid")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("research contract serialization failed: {0}")]
    Serialization(String),
}
