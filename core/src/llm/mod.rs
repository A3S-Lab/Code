//! LLM client abstraction layer
//!
//! Provides a unified interface for interacting with LLM providers
//! (Anthropic Claude, OpenAI, Zhipu AI GLM, and OpenAI-compatible providers).

pub mod anthropic;
pub mod factory;
pub mod http;
pub mod openai;
pub mod structured;
mod types;
pub mod zhipu;

// Re-export public types
pub use anthropic::AnthropicClient;
pub use factory::{create_client_with_config, LlmConfig};
pub use http::{
    clear_http_metrics_callback, default_http_client, set_http_metrics_callback, HttpClient,
    HttpMetricsCallback, HttpMetricsRecord, HttpResponse, StreamingHttpResponse,
};
pub use openai::OpenAiClient;
pub use types::*;
pub use zhipu::ZhipuClient;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// LLM client trait
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Complete a conversation (non-streaming)
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse>;

    /// Complete a conversation with streaming
    /// Returns a receiver for streaming events.
    /// The cancel_token is checked during the HTTP request; if cancelled, the request is aborted.
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>>;
}

// Include test modules — these reference internal types via crate paths
#[cfg(test)]
#[path = "tests.rs"]
mod tests_file;
