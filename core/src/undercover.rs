//! Undercover mode for safe public repository operations
//!
//! When active, adds safety instructions to commit/PR prompts and strips
//! attribution to avoid leaking internal model codenames, project names,
//! or other internal information.
//!
//! ## Activation
//!
//! - `A3S_UNDERCOVER=1` — force ON (even in internal repos)
//! - Otherwise AUTO: active UNLESS the repo remote matches allowlist
//! - There is NO force-OFF — safe default

use crate::prompts::UNDERCOVER_INSTRUCTIONS;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Undercover mode status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndercoverStatus {
    /// Actively operating undercover
    Active,
    /// Not undercover
    Inactive,
    /// Status not yet determined
    Unknown,
}

/// Internal repository classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoClass {
    /// Internal A3S repository
    Internal,
    /// Public/open-source repository
    External,
    /// Not a git repository or cannot determine
    None,
}

/// Undercover service for managing safe operations
pub struct UndercoverService {
    /// Current status
    status: RwLock<UndercoverStatus>,
    /// Internal repo allowlist
    internal_domains: Vec<String>,
    /// User instructions to inject
    instructions: String,
}

impl UndercoverService {
    /// Create a new undercover service with default configuration
    pub fn new() -> Self {
        Self::with_internal_domains(vec![
            "github.com/A3S-Lab".to_string(),
            "github.com/anthropics".to_string(),
        ])
    }

    /// Create with custom internal domain list
    pub fn with_internal_domains(domains: Vec<String>) -> Self {
        Self {
            status: RwLock::new(UndercoverStatus::Unknown),
            internal_domains: domains,
            instructions: UNDERCOVER_INSTRUCTIONS.to_string(),
        }
    }

    /// Determine if undercover mode is active for the given repo
    pub fn is_active(&self, repo_path: &PathBuf) -> bool {
        // Check force env var first
        if std::env::var("A3S_UNDERCOVER")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            *self.status.write().unwrap() = UndercoverStatus::Active;
            return true;
        }

        // Classify the repository
        let class = self.classify_repo(repo_path);

        let active = class != RepoClass::Internal;
        *self.status.write().unwrap() = if active {
            UndercoverStatus::Active
        } else {
            UndercoverStatus::Inactive
        };

        active
    }

    /// Classify a repository as internal or external
    pub fn classify_repo(&self, repo_path: &PathBuf) -> RepoClass {
        let git_dir = repo_path.join(".git");
        if !git_dir.exists() {
            return RepoClass::None;
        }

        // Read the remote URL
        let remote_url = Self::get_remote_url(repo_path);
        let Some(url) = remote_url else {
            return RepoClass::None;
        };

        // Check against internal domain allowlist
        for domain in &self.internal_domains {
            if url.contains(domain) {
                return RepoClass::Internal;
            }
        }

        RepoClass::External
    }

    /// Get the remote URL for a repository
    fn get_remote_url(repo_path: &Path) -> Option<String> {
        // Try to read .git/config
        let config_path = repo_path.join(".git").join("config");
        let config = std::fs::read_to_string(&config_path).ok()?;

        // Find [remote "origin"] section and extract url
        let mut in_origin = false;
        for line in config.lines() {
            let line = line.trim();
            if line == "[remote \"origin\"]" {
                in_origin = true;
            } else if line.starts_with('[') && in_origin {
                break;
            } else if in_origin && line.starts_with("url = ") {
                return Some(line[6..].to_string());
            }
        }

        None
    }

    /// Get current status
    pub fn status(&self) -> UndercoverStatus {
        *self.status.read().unwrap()
    }

    /// Get undercover instructions to prepend to commit messages
    pub fn get_instructions(&self) -> String {
        if self.status() == UndercoverStatus::Active {
            self.instructions.clone()
        } else {
            String::new()
        }
    }

    /// Sanitize a commit message by removing internal references
    pub fn sanitize_commit_message(&self, message: &str) -> String {
        if self.status() != UndercoverStatus::Active {
            return message.to_string();
        }

        let mut result = message.to_string();

        // Remove Co-Authored-By lines
        result = result
            .lines()
            .filter(|line| !line.trim().starts_with("Co-Authored-By:"))
            .collect::<Vec<_>>()
            .join("\n");

        // Remove any lines containing internal codenames
        let codename_patterns = [
            "claude-opus",
            "claude-sonnet",
            "claude-haiku",
            "capybara",
            "tengu",
            "claw-code",
            "a3s-code",
        ];

        for pattern in codename_patterns {
            result = result
                .lines()
                .filter(|line| !line.to_lowercase().contains(pattern))
                .collect::<Vec<_>>()
                .join("\n");
        }

        result.trim().to_string()
    }

    /// Check if a string contains internal references
    pub fn contains_internal_refs(&self, text: &str) -> bool {
        let codename_patterns = [
            "claude-opus",
            "claude-sonnet",
            "claude-haiku",
            "capybara",
            "tengu",
            "claw-code",
            "a3s-code",
            "co-authored-by:",
        ];

        let text_lower = text.to_lowercase();
        codename_patterns.iter().any(|p| text_lower.contains(p))
    }
}

impl Default for UndercoverService {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for UndercoverService {
    fn clone(&self) -> Self {
        Self {
            status: RwLock::new(*self.status.read().unwrap()),
            internal_domains: self.internal_domains.clone(),
            instructions: self.instructions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_nonexistent() {
        let service = UndercoverService::new();
        let result = service.classify_repo(&PathBuf::from("/nonexistent/path"));
        assert_eq!(result, RepoClass::None);
    }

    #[test]
    fn test_sanitize_commit_message() {
        let service = UndercoverService::new();
        // Force active status
        *service.status.write().unwrap() = UndercoverStatus::Active;

        let dirty = "Fix bug\nCo-Authored-By: Claude <claude@example.com>\nGenerated with a3s-code";
        let clean = service.sanitize_commit_message(dirty);

        assert!(!clean.contains("Co-Authored-By"));
        assert!(!clean.contains("a3s-code"));
        assert!(clean.contains("Fix bug"));
    }

    #[test]
    fn test_contains_internal_refs() {
        let service = UndercoverService::new();

        assert!(service.contains_internal_refs("Using claude-opus-4-6"));
        assert!(service.contains_internal_refs("Co-Authored-By: Claude"));
        assert!(!service.contains_internal_refs("Fix parser bug"));
    }

    #[test]
    fn test_get_instructions_when_inactive() {
        let service = UndercoverService::new();
        *service.status.write().unwrap() = UndercoverStatus::Inactive;
        assert!(service.get_instructions().is_empty());
    }

    #[test]
    fn test_get_instructions_when_active() {
        let service = UndercoverService::new();
        *service.status.write().unwrap() = UndercoverStatus::Active;
        assert!(!service.get_instructions().is_empty());
        assert!(service.get_instructions().contains("UNDERCOVER MODE"));
    }

    #[test]
    fn test_undercover_service_default() {
        let service = UndercoverService::default();
        assert_eq!(service.status(), UndercoverStatus::Unknown);
    }

    #[test]
    fn test_undercover_service_clone() {
        let service = UndercoverService::new();
        let cloned = service.clone();
        assert_eq!(cloned.status(), service.status());
    }

    #[test]
    fn test_undercover_service_with_custom_domains() {
        let service =
            UndercoverService::with_internal_domains(vec!["github.com/custom".to_string()]);
        // Should not contain the default anthropics domain
        // but should have the custom one
        let result = service.classify_repo(&PathBuf::from("/nonexistent"));
        assert_eq!(result, RepoClass::None);
    }

    #[test]
    fn test_undercover_status_debug() {
        assert_eq!(format!("{:?}", UndercoverStatus::Active), "Active");
        assert_eq!(format!("{:?}", UndercoverStatus::Inactive), "Inactive");
        assert_eq!(format!("{:?}", UndercoverStatus::Unknown), "Unknown");
    }

    #[test]
    fn test_repo_class_debug() {
        assert_eq!(format!("{:?}", RepoClass::Internal), "Internal");
        assert_eq!(format!("{:?}", RepoClass::External), "External");
        assert_eq!(format!("{:?}", RepoClass::None), "None");
    }

    #[test]
    fn test_sanitize_commit_message_preserves_good_content() {
        let service = UndercoverService::new();
        *service.status.write().unwrap() = UndercoverStatus::Active;

        let msg = "Fix race condition in file watcher initialization";
        let clean = service.sanitize_commit_message(msg);
        assert_eq!(clean, msg);
    }

    #[test]
    fn test_sanitize_removes_multiple_internal_refs() {
        let service = UndercoverService::new();
        *service.status.write().unwrap() = UndercoverStatus::Active;

        let dirty = "Fix bug\nCo-Authored-By: Claude\nclaude-opus was used\na3s-code";
        let clean = service.sanitize_commit_message(dirty);

        assert!(!clean.contains("Co-Authored-By"));
        assert!(!clean.contains("claude-opus"));
        assert!(!clean.contains("a3s-code"));
        assert!(clean.contains("Fix bug"));
    }

    #[test]
    fn test_sanitize_ignores_when_inactive() {
        let service = UndercoverService::new();
        *service.status.write().unwrap() = UndercoverStatus::Inactive;

        let msg = "Co-Authored-By: Claude";
        let clean = service.sanitize_commit_message(msg);
        // When inactive, no sanitization happens
        assert_eq!(clean, msg);
    }

    #[test]
    fn test_contains_internal_refs_case_insensitive() {
        let service = UndercoverService::new();

        assert!(service.contains_internal_refs("CLAUDE-OPUS"));
        assert!(service.contains_internal_refs("A3S-CODE"));
        assert!(service.contains_internal_refs("Co-Authored-By:"));
    }

    #[test]
    fn test_contains_internal_refs_all_patterns() {
        let service = UndercoverService::new();

        assert!(service.contains_internal_refs("claude-sonnet"));
        assert!(service.contains_internal_refs("claude-haiku"));
        assert!(service.contains_internal_refs("capybara"));
        assert!(service.contains_internal_refs("tengu"));
        assert!(service.contains_internal_refs("claw-code"));
    }

    #[test]
    fn test_is_active_unknown_repo_returns_true() {
        let service = UndercoverService::new();
        // Non-existent path should return true (safe default)
        let result = service.is_active(&PathBuf::from("/nonexistent/path"));
        assert!(result);
    }
}
