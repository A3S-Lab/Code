//! LLM client abstraction layer
//!
//! Provides a unified interface for interacting with LLM providers
//! (Anthropic Claude, OpenAI, and OpenAI-compatible providers).

pub mod anthropic;
pub mod factory;
pub mod http;
pub mod openai;
mod types;

// Re-export public types
pub use anthropic::AnthropicClient;
pub use factory::{create_client_with_config, LlmConfig};
pub use http::{default_http_client, HttpClient, HttpResponse, StreamingHttpResponse};
pub use openai::OpenAiClient;
pub use types::*;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

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
    /// Returns a receiver for streaming events
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>>;
}

// Include test modules — these reference internal types via crate paths
#[cfg(test)]
#[path = "tests.rs"]
mod tests_file;
