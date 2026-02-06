//! SafeClaw Configuration
//!
//! Defines sensitivity levels, classification rules, redaction strategies,
//! and the main SafeClawConfig for per-session security settings.

use serde::{Deserialize, Serialize};

/// Sensitivity level for classified data
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityLevel {
    /// Public data, no protection needed
    Public,
    /// Normal data, basic protection
    Normal,
    /// Sensitive data (PII, credentials)
    Sensitive,
    /// Highly sensitive data (SSN, credit cards)
    HighlySensitive,
}

impl std::fmt::Display for SensitivityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SensitivityLevel::Public => write!(f, "public"),
            SensitivityLevel::Normal => write!(f, "normal"),
            SensitivityLevel::Sensitive => write!(f, "sensitive"),
            SensitivityLevel::HighlySensitive => write!(f, "highly_sensitive"),
        }
    }
}

/// A rule for classifying sensitive data in text
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationRule {
    /// Human-readable name for this rule
    pub name: String,
    /// Regex pattern to match sensitive data
    pub pattern: String,
    /// Sensitivity level when matched
    pub level: SensitivityLevel,
}

/// Strategy for redacting sensitive data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStrategy {
    /// Replace with asterisks (e.g., "****-****-****-1234")
    Mask,
    /// Remove entirely (replace with "[REDACTED]")
    Remove,
    /// Replace with SHA-256 hash prefix
    Hash,
}

/// Feature toggles for individual SafeClaw components
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Main SafeClaw configuration for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafeClawConfig {
    /// Whether SafeClaw security is enabled
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

impl Default for SafeClawConfig {
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

/// Default PII classification rules
pub fn default_classification_rules() -> Vec<ClassificationRule> {
    vec![
        ClassificationRule {
            name: "credit_card".to_string(),
            pattern: r"\b(?:\d[ -]*?){13,16}\b".to_string(),
            level: SensitivityLevel::HighlySensitive,
        },
        ClassificationRule {
            name: "ssn".to_string(),
            pattern: r"\b\d{3}-\d{2}-\d{4}\b".to_string(),
            level: SensitivityLevel::HighlySensitive,
        },
        ClassificationRule {
            name: "email".to_string(),
            pattern: r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b".to_string(),
            level: SensitivityLevel::Sensitive,
        },
        ClassificationRule {
            name: "phone".to_string(),
            pattern: r"\b(?:\+?1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b".to_string(),
            level: SensitivityLevel::Sensitive,
        },
        ClassificationRule {
            name: "api_key".to_string(),
            pattern: r"\b(?:sk|pk|api|key|token|secret|password)[-_][A-Za-z0-9_-]{16,}\b"
                .to_string(),
            level: SensitivityLevel::HighlySensitive,
        },
    ]
}

/// Default dangerous command patterns
pub fn default_dangerous_commands() -> Vec<String> {
    vec![
        // curl/wget exfiltration patterns
        r"curl\s+.*--data.*@".to_string(),
        r"curl\s+.*-d\s+@".to_string(),
        r"curl\s+.*-F\s+.*=@".to_string(),
        r"wget\s+.*--post-data".to_string(),
        r"wget\s+.*--post-file".to_string(),
        // Piping sensitive data to network commands
        r"cat\s+.*\|\s*curl".to_string(),
        r"cat\s+.*\|\s*wget".to_string(),
        r"cat\s+.*\|\s*nc\s".to_string(),
    ]
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
        assert_eq!(SensitivityLevel::Public.to_string(), "public");
        assert_eq!(SensitivityLevel::HighlySensitive.to_string(), "highly_sensitive");
    }

    #[test]
    fn test_default_config() {
        let config = SafeClawConfig::default();
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
        // Should contain curl exfiltration patterns
        assert!(commands.iter().any(|c| c.contains("curl")));
        assert!(commands.iter().any(|c| c.contains("wget")));
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = SafeClawConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SafeClawConfig = serde_json::from_str(&json).unwrap();
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
        assert_eq!(json, "\"mask\"");

        let remove = RedactionStrategy::Remove;
        let json = serde_json::to_string(&remove).unwrap();
        assert_eq!(json, "\"remove\"");

        let hash = RedactionStrategy::Hash;
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(json, "\"hash\"");
    }
}
