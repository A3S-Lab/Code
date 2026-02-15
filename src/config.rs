//! Configuration module for A3S Code
//!
//! Provides configuration for:
//! - LLM providers and models (defaultProvider, defaultModel, providers)
//! - Directories for dynamic skill and agent loading
//!
//! Configuration is passed programmatically via `CodeConfig::new()` + builder methods.
//! Provider changes made at runtime via gRPC RPCs are persisted to a JSON file.

use crate::llm::LlmConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

fn default_true() -> bool {
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
    /// Available models
    #[serde(default)]
    pub models: Vec<ModelConfig>,
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
}

// ============================================================================
// Storage Configuration
// ============================================================================

/// Session storage backend type
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// In-memory storage (no persistence)
    Memory,
    /// File-based storage (JSON files)
    #[default]
    File,
    /// Custom external storage (Redis, PostgreSQL, etc.)
    ///
    /// Requires a `SessionStore` implementation registered via `SessionManager::with_store()`.
    /// Use `storage_url` in config to pass connection details.
    Custom,
}

// ============================================================================
// Main Configuration
// ============================================================================

/// Configuration for A3S Code
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeConfig {
    /// Default provider name
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Default model ID
    #[serde(default)]
    pub default_model: Option<String>,

    /// Provider configurations
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,

    /// Session storage backend
    #[serde(default)]
    pub storage_backend: StorageBackend,

    /// Sessions directory (for file backend)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_dir: Option<PathBuf>,

    /// Connection URL for custom storage backend (e.g., "redis://localhost:6379", "postgres://user:pass@localhost/a3s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_url: Option<String>,

    /// Directories to scan for skill files (*.md with tool definitions)
    #[serde(default, alias = "skill_dirs")]
    pub skill_dirs: Vec<PathBuf>,

    /// Directories to scan for agent files (*.yaml or *.md)
    #[serde(default, alias = "agent_dirs")]
    pub agent_dirs: Vec<PathBuf>,

    /// Watch directories for changes (hot-reload) - reserved for future use
    #[serde(default, alias = "watch_enabled")]
    pub watch_enabled: bool,
}

impl CodeConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from a JSON file (used for persistence)
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file {}: {}", path.display(), e))?;

        let config: Self = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("Failed to parse config file {}: {}", path.display(), e)
        })?;

        Ok(config)
    }

    /// Save configuration to a JSON file (used for persistence)
    pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|e| {
            anyhow::anyhow!("Failed to serialize config: {}", e)
        })?;

        std::fs::write(path, content).map_err(|e| {
            anyhow::anyhow!("Failed to write config file {}: {}", path.display(), e)
        })?;

        Ok(())
    }

    /// Find a provider by name
    pub fn find_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Get the default provider configuration
    pub fn default_provider_config(&self) -> Option<&ProviderConfig> {
        self.default_provider
            .as_ref()
            .and_then(|name| self.find_provider(name))
    }

    /// Get the default model configuration
    pub fn default_model_config(&self) -> Option<(&ProviderConfig, &ModelConfig)> {
        let provider = self.default_provider_config()?;
        let model_id = self.default_model.as_ref()?;
        let model = provider.find_model(model_id)?;
        Some((provider, model))
    }

    /// Get LlmConfig for the default provider and model
    ///
    /// Returns None if default provider/model is not configured or API key is missing.
    pub fn default_llm_config(&self) -> Option<LlmConfig> {
        let (provider, model) = self.default_model_config()?;
        let api_key = provider.get_api_key(model)?;
        let base_url = provider.get_base_url(model);

        let mut config = LlmConfig::new(&provider.name, &model.id, api_key);
        if let Some(url) = base_url {
            config = config.with_base_url(url);
        }
        Some(config)
    }

    /// Get LlmConfig for a specific provider and model
    ///
    /// Returns None if provider/model is not found or API key is missing.
    pub fn llm_config(&self, provider_name: &str, model_id: &str) -> Option<LlmConfig> {
        let provider = self.find_provider(provider_name)?;
        let model = provider.find_model(model_id)?;
        let api_key = provider.get_api_key(model)?;
        let base_url = provider.get_base_url(model);

        let mut config = LlmConfig::new(&provider.name, &model.id, api_key);
        if let Some(url) = base_url {
            config = config.with_base_url(url);
        }
        Some(config)
    }

    /// List all available models across all providers
    pub fn list_models(&self) -> Vec<(&ProviderConfig, &ModelConfig)> {
        self.providers
            .iter()
            .flat_map(|p| p.models.iter().map(move |m| (p, m)))
            .collect()
    }

    /// Add a skill directory
    pub fn add_skill_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.skill_dirs.push(dir.into());
        self
    }

    /// Add an agent directory
    pub fn add_agent_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.agent_dirs.push(dir.into());
        self
    }

    /// Enable directory watching
    pub fn with_watch(mut self, enabled: bool) -> Self {
        self.watch_enabled = enabled;
        self
    }

    /// Check if any directories are configured
    pub fn has_directories(&self) -> bool {
        !self.skill_dirs.is_empty() || !self.agent_dirs.is_empty()
    }

    /// Check if provider configuration is available
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = CodeConfig::default();
        assert!(config.skill_dirs.is_empty());
        assert!(config.agent_dirs.is_empty());
        assert!(!config.watch_enabled);
        assert!(config.providers.is_empty());
        assert!(config.default_provider.is_none());
        assert!(config.default_model.is_none());
        assert_eq!(config.storage_backend, StorageBackend::File);
        assert!(config.sessions_dir.is_none());
    }

    #[test]
    fn test_storage_backend_default() {
        let backend = StorageBackend::default();
        assert_eq!(backend, StorageBackend::File);
    }

    #[test]
    fn test_storage_backend_serde() {
        // Test serialization
        let memory = StorageBackend::Memory;
        let json = serde_json::to_string(&memory).unwrap();
        assert_eq!(json, "\"memory\"");

        let file = StorageBackend::File;
        let json = serde_json::to_string(&file).unwrap();
        assert_eq!(json, "\"file\"");

        // Test deserialization
        let memory: StorageBackend = serde_json::from_str("\"memory\"").unwrap();
        assert_eq!(memory, StorageBackend::Memory);

        let file: StorageBackend = serde_json::from_str("\"file\"").unwrap();
        assert_eq!(file, StorageBackend::File);
    }

    #[test]
    fn test_config_with_storage_backend() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");

        std::fs::write(
            &config_path,
            r#"{
                "storageBackend": "memory",
                "sessionsDir": "/tmp/sessions"
            }"#,
        )
        .unwrap();

        let config = CodeConfig::from_file(&config_path).unwrap();
        assert_eq!(config.storage_backend, StorageBackend::Memory);
        assert_eq!(config.sessions_dir, Some(PathBuf::from("/tmp/sessions")));
    }

    #[test]
    fn test_config_builder() {
        let config = CodeConfig::new()
            .add_skill_dir("/tmp/skills")
            .add_agent_dir("/tmp/agents")
            .with_watch(true);

        assert_eq!(config.skill_dirs.len(), 1);
        assert_eq!(config.agent_dirs.len(), 1);
        assert!(config.watch_enabled);
    }

    #[test]
    fn test_config_from_json_with_providers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");

        std::fs::write(
            &config_path,
            r#"{
                "defaultProvider": "anthropic",
                "defaultModel": "claude-sonnet-4",
                "providers": [
                    {
                        "name": "anthropic",
                        "apiKey": "test-key",
                        "baseUrl": "https://api.anthropic.com",
                        "models": [
                            {
                                "id": "claude-sonnet-4",
                                "name": "Claude Sonnet 4",
                                "family": "claude-sonnet",
                                "toolCall": true
                            }
                        ]
                    }
                ],
                "skill_dirs": ["/tmp/skills"]
            }"#,
        )
        .unwrap();

        let config = CodeConfig::from_file(&config_path).unwrap();
        assert_eq!(config.default_provider, Some("anthropic".to_string()));
        assert_eq!(config.default_model, Some("claude-sonnet-4".to_string()));
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "anthropic");
        assert_eq!(config.providers[0].models.len(), 1);
        assert_eq!(config.skill_dirs.len(), 1);
    }

    #[test]
    fn test_find_provider() {
        let config = CodeConfig {
            providers: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: Some("key1".to_string()),
                    base_url: None,
                    models: vec![],
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: Some("key2".to_string()),
                    base_url: None,
                    models: vec![],
                },
            ],
            ..Default::default()
        };

        assert!(config.find_provider("anthropic").is_some());
        assert!(config.find_provider("openai").is_some());
        assert!(config.find_provider("unknown").is_none());
    }

    #[test]
    fn test_default_llm_config() {
        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-api-key".to_string()),
                base_url: Some("https://api.anthropic.com".to_string()),
                models: vec![ModelConfig {
                    id: "claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude-sonnet".to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                }],
            }],
            ..Default::default()
        };

        let llm_config = config.default_llm_config().unwrap();
        assert_eq!(llm_config.provider, "anthropic");
        assert_eq!(llm_config.model, "claude-sonnet-4");
        assert_eq!(llm_config.api_key.expose(), "test-api-key");
        assert_eq!(
            llm_config.base_url,
            Some("https://api.anthropic.com".to_string())
        );
    }

    #[test]
    fn test_model_api_key_override() {
        let provider = ProviderConfig {
            name: "openai".to_string(),
            api_key: Some("provider-key".to_string()),
            base_url: Some("https://api.openai.com".to_string()),
            models: vec![
                ModelConfig {
                    id: "gpt-4".to_string(),
                    name: "GPT-4".to_string(),
                    family: "gpt".to_string(),
                    api_key: None, // Uses provider key
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                },
                ModelConfig {
                    id: "custom-model".to_string(),
                    name: "Custom Model".to_string(),
                    family: "custom".to_string(),
                    api_key: Some("model-specific-key".to_string()), // Override
                    base_url: Some("https://custom.api.com".to_string()), // Override
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                },
            ],
        };

        // Model without override uses provider key
        let model1 = provider.find_model("gpt-4").unwrap();
        assert_eq!(provider.get_api_key(model1), Some("provider-key"));
        assert_eq!(
            provider.get_base_url(model1),
            Some("https://api.openai.com")
        );

        // Model with override uses its own key
        let model2 = provider.find_model("custom-model").unwrap();
        assert_eq!(provider.get_api_key(model2), Some("model-specific-key"));
        assert_eq!(
            provider.get_base_url(model2),
            Some("https://custom.api.com")
        );
    }

    #[test]
    fn test_list_models() {
        let config = CodeConfig {
            providers: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: None,
                    base_url: None,
                    models: vec![
                        ModelConfig {
                            id: "claude-1".to_string(),
                            name: "Claude 1".to_string(),
                            family: "claude".to_string(),
                            api_key: None,
                            base_url: None,
                            attachment: false,
                            reasoning: false,
                            tool_call: true,
                            temperature: true,
                            release_date: None,
                            modalities: ModelModalities::default(),
                            cost: ModelCost::default(),
                            limit: ModelLimit::default(),
                        },
                        ModelConfig {
                            id: "claude-2".to_string(),
                            name: "Claude 2".to_string(),
                            family: "claude".to_string(),
                            api_key: None,
                            base_url: None,
                            attachment: false,
                            reasoning: false,
                            tool_call: true,
                            temperature: true,
                            release_date: None,
                            modalities: ModelModalities::default(),
                            cost: ModelCost::default(),
                            limit: ModelLimit::default(),
                        },
                    ],
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: None,
                    base_url: None,
                    models: vec![ModelConfig {
                        id: "gpt-4".to_string(),
                        name: "GPT-4".to_string(),
                        family: "gpt".to_string(),
                        api_key: None,
                        base_url: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: ModelCost::default(),
                        limit: ModelLimit::default(),
                    }],
                },
            ],
            ..Default::default()
        };

        let models = config.list_models();
        assert_eq!(models.len(), 3);
    }

    #[test]
    fn test_config_from_json_missing_fields() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");

        std::fs::write(&config_path, r#"{"skill_dirs": ["/tmp/skills"]}"#).unwrap();

        let config = CodeConfig::from_file(&config_path).unwrap();
        assert_eq!(config.skill_dirs.len(), 1);
        assert!(config.agent_dirs.is_empty());
        assert!(!config.watch_enabled);
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_config_from_file_not_found() {
        let result = CodeConfig::from_file(Path::new("/nonexistent/config.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_config_has_directories() {
        let empty = CodeConfig::default();
        assert!(!empty.has_directories());

        let with_skills = CodeConfig::new().add_skill_dir("/tmp/skills");
        assert!(with_skills.has_directories());

        let with_agents = CodeConfig::new().add_agent_dir("/tmp/agents");
        assert!(with_agents.has_directories());
    }

    #[test]
    fn test_config_has_providers() {
        let empty = CodeConfig::default();
        assert!(!empty.has_providers());

        let with_providers = CodeConfig {
            providers: vec![ProviderConfig {
                name: "test".to_string(),
                api_key: None,
                base_url: None,
                models: vec![],
            }],
            ..Default::default()
        };
        assert!(with_providers.has_providers());
    }

    #[test]
    fn test_storage_backend_equality() {
        assert_eq!(StorageBackend::Memory, StorageBackend::Memory);
        assert_eq!(StorageBackend::File, StorageBackend::File);
        assert_ne!(StorageBackend::Memory, StorageBackend::File);
    }

    #[test]
    fn test_storage_backend_serde_custom() {
        let custom = StorageBackend::Custom;
        // Custom variant is now serializable
        let json = serde_json::to_string(&custom).unwrap();
        assert_eq!(json, "\"custom\"");

        // And deserializable
        let parsed: StorageBackend = serde_json::from_str("\"custom\"").unwrap();
        assert_eq!(parsed, StorageBackend::Custom);
    }

    #[test]
    fn test_model_cost_default() {
        let cost = ModelCost::default();
        assert_eq!(cost.input, 0.0);
        assert_eq!(cost.output, 0.0);
        assert_eq!(cost.cache_read, 0.0);
        assert_eq!(cost.cache_write, 0.0);
    }

    #[test]
    fn test_model_cost_serialization() {
        let cost = ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        };
        let json = serde_json::to_string(&cost).unwrap();
        assert!(json.contains("\"input\":3"));
        assert!(json.contains("\"output\":15"));
    }

    #[test]
    fn test_model_cost_deserialization_missing_fields() {
        let json = r#"{"input":3.0}"#;
        let cost: ModelCost = serde_json::from_str(json).unwrap();
        assert_eq!(cost.input, 3.0);
        assert_eq!(cost.output, 0.0);
        assert_eq!(cost.cache_read, 0.0);
        assert_eq!(cost.cache_write, 0.0);
    }

    #[test]
    fn test_model_limit_default() {
        let limit = ModelLimit::default();
        assert_eq!(limit.context, 0);
        assert_eq!(limit.output, 0);
    }

    #[test]
    fn test_model_limit_serialization() {
        let limit = ModelLimit {
            context: 200000,
            output: 8192,
        };
        let json = serde_json::to_string(&limit).unwrap();
        assert!(json.contains("\"context\":200000"));
        assert!(json.contains("\"output\":8192"));
    }

    #[test]
    fn test_model_limit_deserialization_missing_fields() {
        let json = r#"{"context":100000}"#;
        let limit: ModelLimit = serde_json::from_str(json).unwrap();
        assert_eq!(limit.context, 100000);
        assert_eq!(limit.output, 0);
    }

    #[test]
    fn test_model_modalities_default() {
        let modalities = ModelModalities::default();
        assert!(modalities.input.is_empty());
        assert!(modalities.output.is_empty());
    }

    #[test]
    fn test_model_modalities_serialization() {
        let modalities = ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        };
        let json = serde_json::to_string(&modalities).unwrap();
        assert!(json.contains("\"input\""));
        assert!(json.contains("\"text\""));
    }

    #[test]
    fn test_model_modalities_deserialization_missing_fields() {
        let json = r#"{"input":["text"]}"#;
        let modalities: ModelModalities = serde_json::from_str(json).unwrap();
        assert_eq!(modalities.input.len(), 1);
        assert!(modalities.output.is_empty());
    }

    #[test]
    fn test_model_config_serialization() {
        let config = ModelConfig {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            family: "gpt-4".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            attachment: true,
            reasoning: false,
            tool_call: true,
            temperature: true,
            release_date: Some("2024-05-13".to_string()),
            modalities: ModelModalities::default(),
            cost: ModelCost::default(),
            limit: ModelLimit::default(),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"id\":\"gpt-4o\""));
        assert!(json.contains("\"attachment\":true"));
    }

    #[test]
    fn test_model_config_deserialization_with_defaults() {
        let json = r#"{"id":"test-model"}"#;
        let config: ModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.id, "test-model");
        assert_eq!(config.name, "");
        assert_eq!(config.family, "");
        assert!(config.api_key.is_none());
        assert!(!config.attachment);
        assert!(config.tool_call);
        assert!(config.temperature);
    }

    #[test]
    fn test_model_config_all_optional_fields() {
        let json = r#"{
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "family": "claude-sonnet",
            "apiKey": "sk-test",
            "baseUrl": "https://api.anthropic.com",
            "attachment": true,
            "reasoning": true,
            "toolCall": false,
            "temperature": false,
            "releaseDate": "2025-05-14"
        }"#;
        let config: ModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.id, "claude-sonnet-4");
        assert_eq!(config.name, "Claude Sonnet 4");
        assert_eq!(config.api_key, Some("sk-test".to_string()));
        assert_eq!(
            config.base_url,
            Some("https://api.anthropic.com".to_string())
        );
        assert!(config.attachment);
        assert!(config.reasoning);
        assert!(!config.tool_call);
        assert!(!config.temperature);
    }

    #[test]
    fn test_provider_config_serialization() {
        let provider = ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: Some("https://api.anthropic.com".to_string()),
            models: vec![],
        };
        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("\"name\":\"anthropic\""));
        assert!(json.contains("\"apiKey\":\"sk-test\""));
    }

    #[test]
    fn test_provider_config_deserialization_missing_optional() {
        let json = r#"{"name":"openai"}"#;
        let provider: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(provider.name, "openai");
        assert!(provider.api_key.is_none());
        assert!(provider.base_url.is_none());
        assert!(provider.models.is_empty());
    }

    #[test]
    fn test_provider_config_find_model() {
        let provider = ProviderConfig {
            name: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: ModelCost::default(),
                limit: ModelLimit::default(),
            }],
        };

        let found = provider.find_model("claude-sonnet-4");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "claude-sonnet-4");

        let not_found = provider.find_model("gpt-4o");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_provider_config_get_api_key() {
        let provider = ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("provider-key".to_string()),
            base_url: None,
            models: vec![],
        };

        let model_with_key = ModelConfig {
            id: "test".to_string(),
            name: "".to_string(),
            family: "".to_string(),
            api_key: Some("model-key".to_string()),
            base_url: None,
            attachment: false,
            reasoning: false,
            tool_call: true,
            temperature: true,
            release_date: None,
            modalities: ModelModalities::default(),
            cost: ModelCost::default(),
            limit: ModelLimit::default(),
        };

        let model_without_key = ModelConfig {
            id: "test2".to_string(),
            name: "".to_string(),
            family: "".to_string(),
            api_key: None,
            base_url: None,
            attachment: false,
            reasoning: false,
            tool_call: true,
            temperature: true,
            release_date: None,
            modalities: ModelModalities::default(),
            cost: ModelCost::default(),
            limit: ModelLimit::default(),
        };

        assert_eq!(provider.get_api_key(&model_with_key), Some("model-key"));
        assert_eq!(
            provider.get_api_key(&model_without_key),
            Some("provider-key")
        );
    }

    #[test]
    fn test_code_config_from_file_invalid_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");
        std::fs::write(&config_path, "invalid json {").unwrap();

        let result = CodeConfig::from_file(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_code_config_default_provider_config() {
        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: None,
                models: vec![],
            }],
            ..Default::default()
        };

        let provider = config.default_provider_config();
        assert!(provider.is_some());
        assert_eq!(provider.unwrap().name, "anthropic");
    }

    #[test]
    fn test_code_config_default_model_config() {
        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: None,
                models: vec![ModelConfig {
                    id: "claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude-sonnet".to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                }],
            }],
            ..Default::default()
        };

        let result = config.default_model_config();
        assert!(result.is_some());
        let (provider, model) = result.unwrap();
        assert_eq!(provider.name, "anthropic");
        assert_eq!(model.id, "claude-sonnet-4");
    }

    #[test]
    fn test_code_config_default_llm_config() {
        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: Some("https://api.anthropic.com".to_string()),
                models: vec![ModelConfig {
                    id: "claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude-sonnet".to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                }],
            }],
            ..Default::default()
        };

        let llm_config = config.default_llm_config();
        assert!(llm_config.is_some());
    }

    #[test]
    fn test_code_config_list_models() {
        let config = CodeConfig {
            providers: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: None,
                    base_url: None,
                    models: vec![ModelConfig {
                        id: "claude-sonnet-4".to_string(),
                        name: "".to_string(),
                        family: "".to_string(),
                        api_key: None,
                        base_url: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: ModelCost::default(),
                        limit: ModelLimit::default(),
                    }],
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: None,
                    base_url: None,
                    models: vec![ModelConfig {
                        id: "gpt-4o".to_string(),
                        name: "".to_string(),
                        family: "".to_string(),
                        api_key: None,
                        base_url: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: ModelCost::default(),
                        limit: ModelLimit::default(),
                    }],
                },
            ],
            ..Default::default()
        };

        let models = config.list_models();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_llm_config_specific_provider_model() {
        let model: ModelConfig = serde_json::from_value(serde_json::json!({
            "id": "claude-3",
            "name": "Claude 3"
        }))
        .unwrap();

        let config = CodeConfig {
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: None,
                models: vec![model],
            }],
            ..Default::default()
        };

        let llm = config.llm_config("anthropic", "claude-3");
        assert!(llm.is_some());
        let llm = llm.unwrap();
        assert_eq!(llm.provider, "anthropic");
        assert_eq!(llm.model, "claude-3");
    }

    #[test]
    fn test_llm_config_missing_provider() {
        let config = CodeConfig::default();
        assert!(config.llm_config("nonexistent", "model").is_none());
    }

    #[test]
    fn test_llm_config_missing_model() {
        let config = CodeConfig {
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: None,
                models: vec![],
            }],
            ..Default::default()
        };
        assert!(config.llm_config("anthropic", "nonexistent").is_none());
    }

    #[test]
    fn test_save_to_file_and_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: Some("https://api.anthropic.com".to_string()),
                models: vec![],
            }],
            storage_backend: StorageBackend::Memory,
            ..Default::default()
        };

        config.save_to_file(&config_path).unwrap();

        let loaded = CodeConfig::from_file(&config_path).unwrap();
        assert_eq!(loaded.default_provider, Some("anthropic".to_string()));
        assert_eq!(loaded.default_model, Some("claude-sonnet-4".to_string()));
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].name, "anthropic");
        assert_eq!(loaded.storage_backend, StorageBackend::Memory);
    }

    #[test]
    fn test_save_to_file_creates_parent_dirs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("nested").join("dir").join("config.json");

        let config = CodeConfig::default();
        config.save_to_file(&config_path).unwrap();

        assert!(config_path.exists());
    }
}
