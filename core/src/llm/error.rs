//! Error markers shared by LLM clients and the agent retry loop.

use thiserror::Error;

/// A provider error that cannot succeed through an immediate retry.
///
/// The message must be safe to show directly to an end user. Provider clients
/// should use this only for precise terminal conditions, such as an exhausted
/// account quota, and not for ordinary transient rate limits.
const MAX_PROVIDER_ERROR_MESSAGE_BYTES: usize = 4 * 1024;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct NonRetryableLlmError {
    message: String,
    provider: Option<String>,
    status: Option<u16>,
}

impl NonRetryableLlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: bound_message(message.into()),
            provider: None,
            status: None,
        }
    }

    /// Construct a terminal provider response without relying on rendered
    /// error text for retry decisions. The response body is bounded because
    /// provider gateways may return arbitrarily large diagnostic payloads.
    pub fn from_status(provider: impl Into<String>, status: u16, body: impl Into<String>) -> Self {
        let provider = provider.into();
        let mut error = Self::new(format!(
            "{provider} API returned HTTP {status}: {}",
            body.into()
        ));
        error.provider = Some(provider);
        error.status = Some(status);
        error
    }

    /// Provider label, when the error came from an HTTP response.
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// HTTP status, when the error came from an HTTP response.
    pub fn status(&self) -> Option<u16> {
        self.status
    }
}

fn bound_message(message: String) -> String {
    if message.len() <= MAX_PROVIDER_ERROR_MESSAGE_BYTES {
        return message;
    }
    let mut bounded = String::with_capacity(MAX_PROVIDER_ERROR_MESSAGE_BYTES);
    for character in message.chars() {
        if bounded.len() + character.len_utf8() + 3 > MAX_PROVIDER_ERROR_MESSAGE_BYTES {
            break;
        }
        bounded.push(character);
    }
    bounded.push('…');
    bounded
}

pub(crate) fn non_retryable_llm_error_message(error: &anyhow::Error) -> Option<&str> {
    if let Some(error) = error.downcast_ref::<NonRetryableLlmError>() {
        return Some(error.message.as_str());
    }
    error
        .downcast_ref::<crate::retry::RetryExhaustedError>()
        .map(crate::retry::RetryExhaustedError::non_retryable_message)
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

    #[test]
    fn provider_status_is_typed_and_message_is_bounded() {
        let error = NonRetryableLlmError::from_status("deepseek", 402, "x".repeat(10_000));
        assert_eq!(error.provider(), Some("deepseek"));
        assert_eq!(error.status(), Some(402));
        assert!(error.to_string().len() <= MAX_PROVIDER_ERROR_MESSAGE_BYTES);
        assert!(error.to_string().ends_with('…'));
    }

    #[test]
    fn retry_exhaustion_is_terminal_at_the_agent_boundary() {
        let error = anyhow::Error::new(crate::retry::RetryExhaustedError::new(
            3,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "rate limited",
        ));
        let message = non_retryable_llm_error_message(&error)
            .expect("retry exhaustion must be terminal at the Agent boundary");
        assert!(message.contains("LLM API request failed after 3 attempts"));
        assert!(message.contains("429"));
        assert!(message.contains("rate limited"));
    }
}
