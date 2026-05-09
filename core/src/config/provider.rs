use crate::llm::LlmConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Provider Configuration
// ============================================================================

/// Model cost information (per million tokens)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// Input token cost
    #[serde(default)]
    pub input: f64,
    /// Output token cost
    #[serde(default)]
    pub output: f64,
    /// Cache read cost
    #[serde(default)]
    pub cache_read: f64,
    /// Cache write cost
    #[serde(default)]
    pub cache_write: f64,
}

/// Model limits
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLimit {
    /// Maximum context tokens
    #[serde(default)]
    pub context: u32,
    /// Maximum output tokens
    #[serde(default)]
    pub output: u32,
}

/// Model modalities (input/output types)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelModalities {
    /// Supported input types
    #[serde(default)]
    pub input: Vec<String>,
    /// Supported output types
    #[serde(default)]
    pub output: Vec<String>,
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    /// Model ID (e.g., "claude-sonnet-4-20250514")
    pub id: String,
    /// Display name
    #[serde(default)]
    pub name: String,
    /// Model family (e.g., "claude-sonnet")
    #[serde(default)]
    pub family: String,
    /// Per-model API key override
    #[serde(default)]
    pub api_key: Option<String>,
    /// Per-model base URL override
    #[serde(default)]
    pub base_url: Option<String>,
    /// Static HTTP headers for this model
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Header name to receive the runtime session ID
    #[serde(default)]
    pub session_id_header: Option<String>,
    /// Supports file attachments
    #[serde(default)]
    pub attachment: bool,
    /// Supports reasoning/thinking
    #[serde(default)]
    pub reasoning: bool,
    /// Supports tool calling
    #[serde(default = "default_true")]
    pub tool_call: bool,
    /// Supports temperature setting
    #[serde(default = "default_true")]
    pub temperature: bool,
    /// Release date
    #[serde(default)]
    pub release_date: Option<String>,
    /// Input/output modalities
    #[serde(default)]
    pub modalities: ModelModalities,
    /// Cost information
    #[serde(default)]
    pub cost: ModelCost,
    /// Token limits
    #[serde(default)]
    pub limit: ModelLimit,
}

pub(crate) fn default_true() -> bool {
    true
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Provider name (e.g., "anthropic", "openai")
    pub name: String,
    /// API key for this provider
    #[serde(default)]
    pub api_key: Option<String>,
    /// Base URL for the API
    #[serde(default)]
    pub base_url: Option<String>,
    /// Static HTTP headers for this provider
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Header name to receive the runtime session ID
    #[serde(default)]
    pub session_id_header: Option<String>,
    /// Available models
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

/// Apply model capability flags to an LlmConfig.
///
/// - `temperature = false` → omit temperature (model ignores it, e.g. o1)
/// - `reasoning = true` + `thinking_budget` set → pass budget to client
/// - `limit.output > 0` → use as max_tokens
pub(crate) fn apply_model_caps(
    mut config: LlmConfig,
    model: &ModelConfig,
    thinking_budget: Option<usize>,
) -> LlmConfig {
    // reasoning=true + thinking_budget set → pass budget to client (Anthropic only)
    if model.reasoning {
        if let Some(budget) = thinking_budget {
            config = config.with_thinking_budget(budget);
        }
    }

    // limit.output > 0 → use as max_tokens cap
    if model.limit.output > 0 {
        config = config.with_max_tokens(model.limit.output as usize);
    }

    // temperature=false models (e.g. o1) must not receive a temperature param.
    // Store the flag so the LLM client can gate it at call time.
    if !model.temperature {
        config.disable_temperature = true;
    }

    config
}

impl ProviderConfig {
    /// Find a model by ID
    pub fn find_model(&self, model_id: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == model_id)
    }

    /// Get the effective API key for a model (model override or provider default)
    pub fn get_api_key<'a>(&'a self, model: &'a ModelConfig) -> Option<&'a str> {
        model.api_key.as_deref().or(self.api_key.as_deref())
    }

    /// Get the effective base URL for a model (model override or provider default)
    pub fn get_base_url<'a>(&'a self, model: &'a ModelConfig) -> Option<&'a str> {
        model.base_url.as_deref().or(self.base_url.as_deref())
    }

    /// Get the effective static headers for a model (provider defaults with model overrides)
    pub fn get_headers(&self, model: &ModelConfig) -> HashMap<String, String> {
        let mut headers = self.headers.clone();
        headers.extend(model.headers.clone());
        headers
    }

    /// Get the header name that should carry the runtime session ID.
    pub fn get_session_id_header<'a>(&'a self, model: &'a ModelConfig) -> Option<&'a str> {
        model
            .session_id_header
            .as_deref()
            .or(self.session_id_header.as_deref())
    }
}
