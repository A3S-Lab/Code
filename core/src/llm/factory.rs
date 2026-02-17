//! LLM client factory

use super::anthropic::AnthropicClient;
use super::openai::OpenAiClient;
use super::types::SecretString;
use super::LlmClient;
use crate::retry::RetryConfig;
use std::sync::Arc;

/// LLM client configuration
#[derive(Clone, Default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key: SecretString,
    pub base_url: Option<String>,
    pub retry_config: Option<RetryConfig>,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("retry_config", &self.retry_config)
            .finish()
    }
}

impl LlmConfig {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            api_key: SecretString::new(api_key.into()),
            base_url: None,
            retry_config: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = Some(retry_config);
        self
    }
}

/// Create LLM client with full configuration (supports custom base_url)
pub fn create_client_with_config(config: LlmConfig) -> Arc<dyn LlmClient> {
    let retry = config.retry_config.unwrap_or_default();
    let api_key = config.api_key.expose().to_string();

    match config.provider.as_str() {
        "anthropic" | "claude" => {
            let mut client = AnthropicClient::new(api_key, config.model).with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
        "openai" | "gpt" => {
            let mut client = OpenAiClient::new(api_key, config.model).with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
        // OpenAI-compatible providers (deepseek, groq, together, ollama, etc.)
        _ => {
            tracing::info!(
                "Using OpenAI-compatible client for provider '{}'",
                config.provider
            );
            let mut client = OpenAiClient::new(api_key, config.model).with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
    }
}
