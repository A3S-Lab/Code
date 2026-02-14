//! Security Configuration
//!
//! Re-exports shared types from `a3s-privacy` and defines the per-session
//! SecurityConfig for security settings within a3s-code.

// Re-export from shared a3s-privacy crate (single source of truth)
pub use a3s_privacy::SensitivityLevel;
pub use a3s_privacy::ClassificationRule;
pub use a3s_privacy::RedactionStrategy;
pub use a3s_privacy::{default_classification_rules, default_dangerous_commands};

/// Feature toggles for individual Security components
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeatureToggles {
    /// Enable output sanitization
    pub output_sanitizer: bool,
    /// Enable taint tracking
    pub taint_tracking: bool,
    /// Enable tool interception
    pub tool_interceptor: bool,
    /// Enable prompt injection detection
    pub injection_defense: bool,
}

impl Default for FeatureToggles {
    fn default() -> Self {
        Self {
            output_sanitizer: true,
            taint_tracking: true,
            tool_interceptor: true,
            injection_defense: true,
        }
    }
}

/// Main Security configuration for a session
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecurityConfig {
    /// Whether Security security is enabled
    pub enabled: bool,
    /// Classification rules for detecting sensitive data
    pub classification_rules: Vec<ClassificationRule>,
    /// How to redact detected sensitive data
    pub redaction_strategy: RedactionStrategy,
    /// Allowed network destinations (for tool interception)
    pub network_whitelist: Vec<String>,
    /// Dangerous command patterns (regex) to block
    pub dangerous_commands: Vec<String>,
    /// Feature toggles for individual components
    pub features: FeatureToggles,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            classification_rules: default_classification_rules(),
            redaction_strategy: RedactionStrategy::Remove,
            network_whitelist: Vec::new(),
            dangerous_commands: default_dangerous_commands(),
            features: FeatureToggles::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitivity_level_ordering() {
        assert!(SensitivityLevel::Public < SensitivityLevel::Normal);
        assert!(SensitivityLevel::Normal < SensitivityLevel::Sensitive);
        assert!(SensitivityLevel::Sensitive < SensitivityLevel::HighlySensitive);
    }

    #[test]
    fn test_sensitivity_level_display() {
        assert_eq!(SensitivityLevel::Public.to_string(), "Public");
        assert_eq!(
            SensitivityLevel::HighlySensitive.to_string(),
            "HighlySensitive"
        );
    }

    #[test]
    fn test_default_config() {
        let config = SecurityConfig::default();
        assert!(config.enabled);
        assert!(!config.classification_rules.is_empty());
        assert_eq!(config.redaction_strategy, RedactionStrategy::Remove);
        assert!(!config.dangerous_commands.is_empty());
        assert!(config.features.output_sanitizer);
    }

    #[test]
    fn test_default_classification_rules() {
        let rules = default_classification_rules();
        assert_eq!(rules.len(), 5);

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"credit_card"));
        assert!(names.contains(&"ssn"));
        assert!(names.contains(&"email"));
        assert!(names.contains(&"phone"));
        assert!(names.contains(&"api_key"));
    }

    #[test]
    fn test_default_dangerous_commands() {
        let commands = default_dangerous_commands();
        assert!(!commands.is_empty());
        // Should contain destructive command patterns
        assert!(commands.iter().any(|c| c.contains("rm")));
        assert!(commands.iter().any(|c| c.contains("dd")));
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = SecurityConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SecurityConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.redaction_strategy, config.redaction_strategy);
        assert_eq!(
            parsed.classification_rules.len(),
            config.classification_rules.len()
        );
    }

    #[test]
    fn test_redaction_strategy_serialization() {
        let mask = RedactionStrategy::Mask;
        let json = serde_json::to_string(&mask).unwrap();
        assert_eq!(json, "\"Mask\"");

        let remove = RedactionStrategy::Remove;
        let json = serde_json::to_string(&remove).unwrap();
        assert_eq!(json, "\"Remove\"");

        let hash = RedactionStrategy::Hash;
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(json, "\"Hash\"");
    }
}
