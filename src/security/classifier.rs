//! Security Privacy Classifier
//!
//! Thin wrapper around `a3s_privacy::RegexClassifier` that preserves the
//! existing a3s-code API while delegating to the shared implementation.

use super::config::{ClassificationRule, RedactionStrategy, SensitivityLevel};

// Re-export PiiMatch from shared crate for consumers that need it
pub use a3s_privacy::PiiMatch;

/// Result of classifying a piece of text
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// Overall highest sensitivity level found
    pub overall_level: SensitivityLevel,
    /// All matches found
    pub matches: Vec<PiiMatch>,
}

/// Privacy classifier with pre-compiled regex rules.
///
/// Wraps `a3s_privacy::RegexClassifier` with the a3s-code-specific API.
pub struct PrivacyClassifier {
    inner: a3s_privacy::RegexClassifier,
}

impl PrivacyClassifier {
    /// Create a new classifier from classification rules
    pub fn new(rules: &[ClassificationRule]) -> Self {
        let inner = a3s_privacy::RegexClassifier::new(rules, SensitivityLevel::Public)
            .expect("default rules should always compile");
        Self { inner }
    }

    /// Classify text and return all matches
    pub fn classify(&self, text: &str) -> ClassificationResult {
        let result = self.inner.classify(text);
        ClassificationResult {
            overall_level: result.overall_level,
            matches: result.matches,
        }
    }

    /// Redact all matches in text using the given strategy
    pub fn redact(&self, text: &str, strategy: RedactionStrategy) -> String {
        self.inner.redact(text, strategy)
    }

    /// Quick check: does the text contain any sensitive data?
    pub fn contains_sensitive(&self, text: &str) -> bool {
        self.inner.contains_sensitive(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::config::default_classification_rules;

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
        // The phone regex expects digit-only prefix: \b\d{3}[-.\s]?\d{3}[-.\s]?\d{4}\b
        let result = classifier.classify("Call me at 555-123-4567");
        assert!(!result.matches.is_empty());
    }

    #[test]
    fn test_detect_api_key() {
        let classifier = make_classifier();
        // The api_key regex requires 32+ alphanumeric/dash/underscore chars
        let result = classifier.classify("Use key sk_test_0123456789abcdefghijklmnop");
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
        // Remove strategy replaces with empty string
        assert!(!redacted.contains("[REDACTED]"));
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
