//! Error markers shared by LLM clients and the agent retry loop.

use thiserror::Error;

/// A provider error that cannot succeed through an immediate retry.
///
/// The message must be safe to show directly to an end user. Provider clients
/// should use this only for precise terminal conditions, such as an exhausted
/// account quota, and not for ordinary transient rate limits.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct NonRetryableLlmError {
    message: String,
}

impl NonRetryableLlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub(crate) fn non_retryable_llm_error_message(error: &anyhow::Error) -> Option<&str> {
    error
        .downcast_ref::<NonRetryableLlmError>()
        .map(|error| error.message.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_survives_anyhow_context() {
        let error = anyhow::Error::new(NonRetryableLlmError::new("quota exhausted"))
            .context("LLM call failed");

        assert_eq!(
            non_retryable_llm_error_message(&error),
            Some("quota exhausted")
        );
    }
}
