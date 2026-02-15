//! LSP Manager
//!
//! Manages language server lifecycle and routing.

use crate::lsp::client::LspClient;
use crate::lsp::servers::{builtin_servers, language_for_extension, LanguageServerConfig};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// LSP server status
#[derive(Debug, Clone)]
pub struct LspServerStatus {
    pub language: String,
    pub connected: bool,
    pub command: String,
}

/// LSP Manager for managing multiple language servers
pub struct LspManager {
    /// Running language servers
    servers: RwLock<HashMap<String, Arc<LspClient>>>,
    /// Server configurations
    configs: RwLock<HashMap<String, LanguageServerConfig>>,
    /// Workspace root
    workspace_root: RwLock<Option<String>>,
}

impl LspManager {
    /// Create a new LSP manager
    pub fn new() -> Self {
        let mut configs = HashMap::new();

        // Load built-in server configurations
        for (lang, config) in builtin_servers() {
            configs.insert(lang, config);
        }

        Self {
            servers: RwLock::new(HashMap::new()),
            configs: RwLock::new(configs),
            workspace_root: RwLock::new(None),
        }
    }

    /// Set workspace root
    pub async fn set_workspace(&self, root: &str) {
        let mut workspace = self.workspace_root.write().await;
        *workspace = Some(root.to_string());
    }

    /// Get workspace root URI
    pub async fn workspace_uri(&self) -> Option<String> {
        let workspace = self.workspace_root.read().await;
        workspace.as_ref().map(|p| format!("file://{}", p))
    }

    /// Register a custom server configuration
    pub async fn register_server(&self, language: String, config: LanguageServerConfig) {
        let mut configs = self.configs.write().await;
        configs.insert(language, config);
    }

    /// Start a language server
    pub async fn start_server(&self, language: &str) -> Result<()> {
        // Check if already running
        {
            let servers = self.servers.read().await;
            if servers.contains_key(language) {
                return Ok(());
            }
        }

        // Get config
        let config = {
            let configs = self.configs.read().await;
            configs
                .get(language)
                .cloned()
                .ok_or_else(|| anyhow!("No configuration for language: {}", language))?
        };

        // Spawn client
        let client = LspClient::spawn(
            language.to_string(),
            &config.command,
            &config.args,
            &config.env,
        )
        .await?;

        // Initialize
        let workspace_uri = self
            .workspace_uri()
            .await
            .unwrap_or_else(|| "file:///".to_string());
        client.initialize(&workspace_uri).await?;

        // Store client
        {
            let mut servers = self.servers.write().await;
            servers.insert(language.to_string(), Arc::new(client));
        }

        tracing::info!("Started LSP server for {}", language);
        Ok(())
    }

    /// Stop a language server
    pub async fn stop_server(&self, language: &str) -> Result<()> {
        let client = {
            let mut servers = self.servers.write().await;
            servers.remove(language)
        };

        if let Some(client) = client {
            client.close().await?;
            tracing::info!("Stopped LSP server for {}", language);
        }

        Ok(())
    }

    /// Get client for a language
    pub async fn get_client(&self, language: &str) -> Option<Arc<LspClient>> {
        let servers = self.servers.read().await;
        servers.get(language).cloned()
    }

    /// Ensure server is running for a file
    pub async fn ensure_server_for_file(&self, path: &Path) -> Result<Arc<LspClient>> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow!("No file extension"))?;

        let language = language_for_extension(ext)
            .ok_or_else(|| anyhow!("No language server for extension: {}", ext))?;

        // Start server if not running
        self.start_server(language).await?;

        // Get client
        self.get_client(language)
            .await
            .ok_or_else(|| anyhow!("Failed to get client for {}", language))
    }

    /// Get status of all servers
    pub async fn get_status(&self) -> Vec<LspServerStatus> {
        let servers = self.servers.read().await;
        let configs = self.configs.read().await;

        let mut status = Vec::new();

        for (language, config) in configs.iter() {
            let connected = servers
                .get(language)
                .map(|c| c.is_connected())
                .unwrap_or(false);
            status.push(LspServerStatus {
                language: language.clone(),
                connected,
                command: config.command.clone(),
            });
        }

        status
    }

    /// List running servers
    pub async fn list_running(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers.keys().cloned().collect()
    }

    /// Stop all servers
    pub async fn stop_all(&self) -> Result<()> {
        let languages: Vec<String> = {
            let servers = self.servers.read().await;
            servers.keys().cloned().collect()
        };

        for language in languages {
            self.stop_server(&language).await?;
        }

        Ok(())
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsp_manager_new() {
        let manager = LspManager::new();
        let status = manager.get_status().await;
        assert!(!status.is_empty());
    }

    #[tokio::test]
    async fn test_lsp_manager_has_builtin_configs() {
        let manager = LspManager::new();
        let status = manager.get_status().await;

        let languages: Vec<&str> = status.iter().map(|s| s.language.as_str()).collect();
        assert!(languages.contains(&"rust"));
        assert!(languages.contains(&"go"));
        assert!(languages.contains(&"typescript"));
        assert!(languages.contains(&"python"));
    }

    #[tokio::test]
    async fn test_lsp_manager_set_workspace() {
        let manager = LspManager::new();
        manager.set_workspace("/workspace").await;

        let uri = manager.workspace_uri().await;
        assert_eq!(uri, Some("file:///workspace".to_string()));
    }

    #[tokio::test]
    async fn test_lsp_manager_register_custom() {
        let manager = LspManager::new();

        let config = LanguageServerConfig {
            command: "custom-lsp".to_string(),
            args: vec![],
            env: HashMap::new(),
            extensions: vec!["custom".to_string()],
            language_id: "custom".to_string(),
            initialization_options: None,
        };

        manager.register_server("custom".to_string(), config).await;

        let status = manager.get_status().await;
        assert!(status.iter().any(|s| s.language == "custom"));
    }
}
