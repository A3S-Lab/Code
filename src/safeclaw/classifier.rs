//! SafeClaw Privacy Classifier
//!
//! Scans text for PII and sensitive data using pre-compiled regex rules.
//! Returns match positions and sensitivity levels for redaction.

use super::config::{ClassificationRule, RedactionStrategy, SensitivityLevel};
use regex::Regex;

/// A pre-compiled classification rule
pub struct CompiledRule {
    /// Rule name
    pub name: String,
    /// Compiled regex
    pub regex: Regex,
    /// Sensitivity level
    pub level: SensitivityLevel,
}

/// A single PII match found in text
#[derive(Debug, Clone)]
pub struct PiiMatch {
    /// Name of the rule that matched
    pub rule_name: String,
    /// Sensitivity level
    pub level: SensitivityLevel,
    /// Start byte position in the text
    pub start: usize,
    /// End byte position in the text
    pub end: usize,
    /// The matched text
    pub matched_text: String,
}

/// Result of classifying a piece of text
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// Overall highest sensitivity level found
    pub overall_level: SensitivityLevel,
    /// All matches found
    pub matches: Vec<PiiMatch>,
}

/// Privacy classifier with pre-compiled regex rules
pub struct PrivacyClassifier {
    rules: Vec<CompiledRule>,
}

impl PrivacyClassifier {
    /// Create a new classifier from classification rules
    pub fn new(rules: &[ClassificationRule]) -> Self {
        let compiled = rules
            .iter()
            .filter_map(|rule| {
                Regex::new(&rule.pattern).ok().map(|regex| CompiledRule {
                    name: rule.name.clone(),
                    regex,
                    level: rule.level,
                })
            })
            .collect();

        Self { rules: compiled }
    }

    /// Classify text and return all matches
    pub fn classify(&self, text: &str) -> ClassificationResult {
        let mut matches = Vec::new();
        let mut overall_level = SensitivityLevel::Public;

        for rule in &self.rules {
            for m in rule.regex.find_iter(text) {
                if rule.level > overall_level {
                    overall_level = rule.level;
                }
                matches.push(PiiMatch {
                    rule_name: rule.name.clone(),
                    level: rule.level,
                    start: m.start(),
                    end: m.end(),
                    matched_text: m.as_str().to_string(),
                });
            }
        }

        ClassificationResult {
            overall_level,
            matches,
        }
    }

    /// Redact all matches in text using the given strategy
    pub fn redact(&self, text: &str, strategy: RedactionStrategy) -> String {
        let result = self.classify(text);
        if result.matches.is_empty() {
            return text.to_string();
        }

        // Sort matches by start position (descending) to replace from end
        let mut sorted_matches = result.matches;
        sorted_matches.sort_by(|a, b| b.start.cmp(&a.start));

        let mut redacted = text.to_string();
        for m in sorted_matches {
            let replacement = match strategy {
                RedactionStrategy::Mask => {
                    let len = m.end - m.start;
                    "*".repeat(len)
                }
                RedactionStrategy::Remove => "[REDACTED]".to_string(),
                RedactionStrategy::Hash => {
                    // Simple hash: use first 8 chars of hex-encoded bytes
                    let hash: String = m
                        .matched_text
                        .bytes()
                        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
                        .to_string();
                    format!("[HASH:{}]", &hash[..hash.len().min(8)])
                }
            };
            redacted.replace_range(m.start..m.end, &replacement);
        }

        redacted
    }

    /// Quick check: does the text contain any sensitive data?
    pub fn contains_sensitive(&self, text: &str) -> bool {
        self.rules.iter().any(|rule| rule.regex.is_match(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safeclaw::config::default_classification_rules;

    fn make_classifier() -> PrivacyClassifier {
        PrivacyClassifier::new(&default_classification_rules())
    }

    #[test]
    fn test_detect_credit_card() {
        let classifier = make_classifier();
        let result = classifier.classify("My card is 4111-1111-1111-1111");
        assert!(!result.matches.is_empty());
        assert!(result.overall_level >= SensitivityLevel::HighlySensitive);
    }

    #[test]
    fn test_detect_ssn() {
        let classifier = make_classifier();
        let result = classifier.classify("SSN: 123-45-6789");
        assert!(!result.matches.is_empty());
        let ssn_match = result.matches.iter().find(|m| m.rule_name == "ssn");
        assert!(ssn_match.is_some());
    }

    #[test]
    fn test_detect_email() {
        let classifier = make_classifier();
        let result = classifier.classify("Contact me at user@example.com");
        assert!(!result.matches.is_empty());
        let email_match = result.matches.iter().find(|m| m.rule_name == "email");
        assert!(email_match.is_some());
    }

    #[test]
    fn test_detect_phone() {
        let classifier = make_classifier();
        let result = classifier.classify("Call me at (555) 123-4567");
        assert!(!result.matches.is_empty());
    }

    #[test]
    fn test_detect_api_key() {
        let classifier = make_classifier();
        let result = classifier.classify("Use key sk_test_0123456789abcdefghij");
        assert!(!result.matches.is_empty());
        assert!(result.overall_level >= SensitivityLevel::HighlySensitive);
    }

    #[test]
    fn test_clean_text_no_matches() {
        let classifier = make_classifier();
        let result = classifier.classify("Hello, this is a normal message.");
        assert!(result.matches.is_empty());
        assert_eq!(result.overall_level, SensitivityLevel::Public);
    }

    #[test]
    fn test_redact_remove() {
        let classifier = make_classifier();
        let redacted = classifier.redact("SSN: 123-45-6789", RedactionStrategy::Remove);
        assert!(!redacted.contains("123-45-6789"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_mask() {
        let classifier = make_classifier();
        let redacted = classifier.redact("SSN: 123-45-6789", RedactionStrategy::Mask);
        assert!(!redacted.contains("123-45-6789"));
        assert!(redacted.contains("***"));
    }

    #[test]
    fn test_redact_hash() {
        let classifier = make_classifier();
        let redacted = classifier.redact("SSN: 123-45-6789", RedactionStrategy::Hash);
        assert!(!redacted.contains("123-45-6789"));
        assert!(redacted.contains("[HASH:"));
    }

    #[test]
    fn test_contains_sensitive() {
        let classifier = make_classifier();
        assert!(classifier.contains_sensitive("SSN: 123-45-6789"));
        assert!(!classifier.contains_sensitive("Hello world"));
    }

    #[test]
    fn test_multiple_matches() {
        let classifier = make_classifier();
        let result = classifier.classify("SSN: 123-45-6789, email: test@example.com");
        assert!(result.matches.len() >= 2);
        assert_eq!(result.overall_level, SensitivityLevel::HighlySensitive);
    }
}
