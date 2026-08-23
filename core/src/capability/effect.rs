use async_trait::async_trait;
use thiserror::Error;

const MAX_EFFECT_ERROR_BYTES: usize = 1_024;

/// Bounded failure returned by asynchronous capability teardown.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct CapabilityEffectError {
    message: Box<str>,
}

impl CapabilityEffectError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let message = if message.is_empty() {
            "capability effect teardown failed".to_owned()
        } else {
            truncate_utf8(message, MAX_EFFECT_ERROR_BYTES)
        };
        Self {
            message: message.into_boxed_str(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// One reversible resource owned by exactly one capability scope.
///
/// Implementations must make `close` idempotent at the underlying resource
/// boundary. The supervisor calls effects in reverse registration order and
/// keeps proceeding after an individual failure. Teardown must use asynchronous
/// I/O and yield normally; blocking a Tokio worker violates the scope close
/// contract.
#[async_trait]
pub trait CapabilityEffect: Send + 'static {
    fn name(&self) -> &str;

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError>;
}

fn truncate_utf8(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut boundary = max;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}
