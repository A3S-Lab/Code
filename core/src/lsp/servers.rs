//! Language Server Configurations
//!
//! Built-in configurations for common language servers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Language server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageServerConfig {
    /// Command to start the server
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// File extensions this server handles
    pub extensions: Vec<String>,
    /// Language ID for LSP
    pub language_id: String,
    /// Initialization options
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<serde_json::Value>,
}

/// Get built-in language server configurations
pub fn builtin_servers() -> HashMap<String, LanguageServerConfig> {
    let mut servers = HashMap::new();

    // Rust - rust-analyzer
    servers.insert(
        "rust".to_string(),
        LanguageServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            env: HashMap::new(),
            extensions: vec!["rs".to_string()],
            language_id: "rust".to_string(),
            initialization_options: None,
        },
    );

    // Go - gopls
    servers.insert(
        "go".to_string(),
        LanguageServerConfig {
            command: "gopls".to_string(),
            args: vec![],
            env: HashMap::new(),
            extensions: vec!["go".to_string()],
            language_id: "go".to_string(),
            initialization_options: None,
        },
    );

    // TypeScript/JavaScript - typescript-language-server
    servers.insert(
        "typescript".to_string(),
        LanguageServerConfig {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            env: HashMap::new(),
            extensions: vec![
                "ts".to_string(),
                "tsx".to_string(),
                "js".to_string(),
                "jsx".to_string(),
            ],
            language_id: "typescript".to_string(),
            initialization_options: None,
        },
    );

    // Python - pyright
    servers.insert(
        "python".to_string(),
        LanguageServerConfig {
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            env: HashMap::new(),
            extensions: vec!["py".to_string()],
            language_id: "python".to_string(),
            initialization_options: None,
        },
    );

    // C/C++ - clangd
    servers.insert(
        "cpp".to_string(),
        LanguageServerConfig {
            command: "clangd".to_string(),
            args: vec![],
            env: HashMap::new(),
            extensions: vec![
                "c".to_string(),
                "cpp".to_string(),
                "cc".to_string(),
                "cxx".to_string(),
                "h".to_string(),
                "hpp".to_string(),
            ],
            language_id: "cpp".to_string(),
            initialization_options: None,
        },
    );

    servers
}

/// Map file extension to language
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "rs" => Some("rust"),
        "go" => Some("go"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" => Some("typescript"),
        "py" => Some("python"),
        "c" | "h" => Some("cpp"),
        "cpp" | "cc" | "cxx" | "hpp" => Some("cpp"),
        _ => None,
    }
}

/// Get language ID for LSP from file extension
pub fn language_id_for_extension(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "rs" => Some("rust"),
        "go" => Some("go"),
        "ts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "py" => Some("python"),
        "c" => Some("c"),
        "h" => Some("c"),
        "cpp" | "cc" | "cxx" => Some("cpp"),
        "hpp" => Some("cpp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_servers() {
        let servers = builtin_servers();
        assert!(servers.contains_key("rust"));
        assert!(servers.contains_key("go"));
        assert!(servers.contains_key("typescript"));
        assert!(servers.contains_key("python"));
        assert!(servers.contains_key("cpp"));
    }

    #[test]
    fn test_language_for_extension() {
        assert_eq!(language_for_extension("rs"), Some("rust"));
        assert_eq!(language_for_extension("go"), Some("go"));
        assert_eq!(language_for_extension("ts"), Some("typescript"));
        assert_eq!(language_for_extension("py"), Some("python"));
        assert_eq!(language_for_extension("cpp"), Some("cpp"));
        assert_eq!(language_for_extension("unknown"), None);
    }

    #[test]
    fn test_language_id_for_extension() {
        assert_eq!(language_id_for_extension("rs"), Some("rust"));
        assert_eq!(language_id_for_extension("tsx"), Some("typescriptreact"));
        assert_eq!(language_id_for_extension("jsx"), Some("javascriptreact"));
    }

    #[test]
    fn test_rust_analyzer_config() {
        let servers = builtin_servers();
        let rust = servers.get("rust").unwrap();
        assert_eq!(rust.command, "rust-analyzer");
        assert!(rust.extensions.contains(&"rs".to_string()));
    }
}
