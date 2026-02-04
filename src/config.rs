//! Configuration module for dynamic skill and agent loading
//!
//! Provides configuration for specifying directories from which skills (tools)
//! and subagents are automatically loaded at runtime.
//!
//! ## Configuration Sources
//!
//! Configuration can be loaded from:
//! - Environment variables (A3S_SKILL_DIRS, A3S_AGENT_DIRS, A3S_WATCH_DIRS)
//! - JSON config file (~/.a3s/config.json)
//!
//! ## Example Config File
//!
//! ```json
//! {
//!   "skill_dirs": ["~/.a3s/skills", "/opt/a3s/skills"],
//!   "agent_dirs": ["~/.a3s/agents", "/opt/a3s/agents"],
//!   "watch_enabled": false
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Configuration for dynamic skill and agent loading
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeConfig {
    /// Directories to scan for skill files (*.md with tool definitions)
    #[serde(default)]
    pub skill_dirs: Vec<PathBuf>,

    /// Directories to scan for agent files (*.yaml or *.md)
    #[serde(default)]
    pub agent_dirs: Vec<PathBuf>,

    /// Watch directories for changes (hot-reload) - reserved for future use
    #[serde(default)]
    pub watch_enabled: bool,
}

impl CodeConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from environment variables
    ///
    /// Environment variables:
    /// - `A3S_SKILL_DIRS`: Colon-separated list of skill directories
    /// - `A3S_AGENT_DIRS`: Colon-separated list of agent directories
    /// - `A3S_WATCH_DIRS`: Enable directory watching (true/false)
    pub fn from_env() -> Self {
        let skill_dirs = std::env::var("A3S_SKILL_DIRS")
            .map(|s| parse_path_list(&s))
            .unwrap_or_default();

        let agent_dirs = std::env::var("A3S_AGENT_DIRS")
            .map(|s| parse_path_list(&s))
            .unwrap_or_default();

        let watch_enabled = std::env::var("A3S_WATCH_DIRS")
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false);

        Self {
            skill_dirs,
            agent_dirs,
            watch_enabled,
        }
    }

    /// Load configuration from a JSON file
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!("Failed to read config file {}: {}", path.display(), e)
        })?;

        let mut config: Self = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("Failed to parse config file {}: {}", path.display(), e)
        })?;

        // Expand ~ in paths
        config.skill_dirs = config.skill_dirs.into_iter().map(expand_tilde).collect();
        config.agent_dirs = config.agent_dirs.into_iter().map(expand_tilde).collect();

        Ok(config)
    }

    /// Merge another configuration into this one
    ///
    /// Directories from `other` are appended to existing directories.
    /// `watch_enabled` is OR'd together.
    pub fn merge(&mut self, other: Self) {
        self.skill_dirs.extend(other.skill_dirs);
        self.agent_dirs.extend(other.agent_dirs);
        self.watch_enabled = self.watch_enabled || other.watch_enabled;
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
}

/// Parse a colon-separated list of paths
fn parse_path_list(s: &str) -> Vec<PathBuf> {
    s.split(':')
        .filter(|p| !p.is_empty())
        .map(|p| expand_tilde(PathBuf::from(p)))
        .collect()
}

/// Expand ~ to home directory
fn expand_tilde(path: PathBuf) -> PathBuf {
    if let Some(path_str) = path.to_str() {
        if path_str.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(&path_str[2..]);
            }
        }
    }
    path
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
    fn test_config_merge() {
        let mut config1 = CodeConfig::new()
            .add_skill_dir("/dir1/skills")
            .add_agent_dir("/dir1/agents");

        let config2 = CodeConfig::new()
            .add_skill_dir("/dir2/skills")
            .add_agent_dir("/dir2/agents")
            .with_watch(true);

        config1.merge(config2);

        assert_eq!(config1.skill_dirs.len(), 2);
        assert_eq!(config1.agent_dirs.len(), 2);
        assert!(config1.watch_enabled);
    }

    #[test]
    fn test_parse_path_list() {
        let paths = parse_path_list("/dir1:/dir2:/dir3");
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], PathBuf::from("/dir1"));
        assert_eq!(paths[1], PathBuf::from("/dir2"));
        assert_eq!(paths[2], PathBuf::from("/dir3"));
    }

    #[test]
    fn test_parse_path_list_empty() {
        let paths = parse_path_list("");
        assert!(paths.is_empty());
    }

    #[test]
    fn test_parse_path_list_with_empty_segments() {
        let paths = parse_path_list("/dir1::/dir2:");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_config_from_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.json");

        std::fs::write(
            &config_path,
            r#"{
                "skill_dirs": ["/tmp/skills"],
                "agent_dirs": ["/tmp/agents"],
                "watch_enabled": true
            }"#,
        )
        .unwrap();

        let config = CodeConfig::from_file(&config_path).unwrap();
        assert_eq!(config.skill_dirs.len(), 1);
        assert_eq!(config.agent_dirs.len(), 1);
        assert!(config.watch_enabled);
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
    fn test_expand_tilde() {
        // Test non-tilde path
        let path = expand_tilde(PathBuf::from("/absolute/path"));
        assert_eq!(path, PathBuf::from("/absolute/path"));

        // Test tilde expansion (if HOME is set)
        if std::env::var("HOME").is_ok() {
            let path = expand_tilde(PathBuf::from("~/test"));
            assert!(!path.to_string_lossy().starts_with("~/"));
        }
    }
}
