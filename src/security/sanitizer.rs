//! Security Output Sanitizer
//!
//! Implements HookHandler for GenerateEnd events to scan and redact
//! sensitive data from LLM responses before they reach the user.

use super::audit::{AuditAction, AuditEntry, AuditEventType, AuditLog};
use super::classifier::PrivacyClassifier;
use super::config::{RedactionStrategy, SensitivityLevel};
use super::taint::TaintRegistry;
use crate::hooks::HookEvent;
use crate::hooks::HookHandler;
use crate::hooks::HookResponse;
use std::sync::{Arc, RwLock};

/// Create a replacement string based on the redaction strategy.
///
/// Shared by `OutputSanitizer` and `SecurityGuard` to avoid duplicating redaction logic.
pub(crate) fn make_replacement(original: &str, strategy: RedactionStrategy) -> String {
    match strategy {
        RedactionStrategy::Mask => "*".repeat(original.len()),
        RedactionStrategy::Remove => "[REDACTED]".to_string(),
        RedactionStrategy::Hash => {
            let hash = original
                .bytes()
                .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
            let hash_str = hash.to_string();
            format!(
                "[HASH:{}]",
                &hash_str[..8.min(hash_str.len())]
            )
        }
    }
}

/// Output sanitizer that redacts sensitive data from LLM responses
pub struct OutputSanitizer {
    taint_registry: Arc<RwLock<TaintRegistry>>,
    classifier: Arc<PrivacyClassifier>,
    redaction_strategy: RedactionStrategy,
    audit_log: Arc<AuditLog>,
    session_id: String,
}

impl OutputSanitizer {
    /// Create a new output sanitizer
    pub fn new(
        taint_registry: Arc<RwLock<TaintRegistry>>,
        classifier: Arc<PrivacyClassifier>,
        redaction_strategy: RedactionStrategy,
        audit_log: Arc<AuditLog>,
        session_id: String,
    ) -> Self {
        Self {
            taint_registry,
            classifier,
            redaction_strategy,
            audit_log,
            session_id,
        }
    }

    /// Sanitize text by redacting tainted and classified sensitive data
    pub fn sanitize_text(&self, text: &str) -> String {
        let mut result = text.to_string();
        let mut was_redacted = false;

        // Step 1: Check taint registry for exact matches and encoded variants
        {
            let Ok(registry) = self.taint_registry.read() else {
                tracing::error!("Taint registry lock poisoned — skipping taint-based redaction");
                return result;
            };
            for (_, entry) in registry.entries_iter() {
                // Replace original value
                if result.contains(&entry.original_value) {
                    let replacement = self.make_replacement(&entry.original_value);
                    result = result.replace(&entry.original_value, &replacement);
                    was_redacted = true;
                }
                // Replace encoded variants
                for variant in &entry.variants {
                    if result.contains(variant.as_str()) {
                        let replacement = self.make_replacement(variant);
                        result = result.replace(variant.as_str(), &replacement);
                        was_redacted = true;
                    }
                }
            }
        }

        // Step 2: Run privacy classifier for pattern-based detection
        let classified = self.classifier.classify(&result);
        if !classified.matches.is_empty() {
            result = self.classifier.redact(&result, self.redaction_strategy);
            was_redacted = true;
        }

        if was_redacted {
            self.audit_log.log(AuditEntry {
                timestamp: chrono::Utc::now(),
                session_id: self.session_id.clone(),
                event_type: AuditEventType::OutputRedacted,
                severity: SensitivityLevel::Sensitive,
                details: "Sensitive data redacted from output".to_string(),
                tool_name: None,
                action_taken: AuditAction::Redacted,
            });
        }

        result
    }

    /// Create a replacement string based on the redaction strategy
    fn make_replacement(&self, original: &str) -> String {
        make_replacement(original, self.redaction_strategy)
    }
}

impl HookHandler for OutputSanitizer {
    fn handle(&self, event: &HookEvent) -> HookResponse {
        if let HookEvent::GenerateEnd(e) = event {
            let sanitized = self.sanitize_text(&e.response_text);
            if sanitized != e.response_text {
                HookResponse::continue_with(serde_json::json!({
                    "response_text": sanitized
                }))
            } else {
                HookResponse::continue_()
            }
        } else {
            HookResponse::continue_()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::config::default_classification_rules;

    fn make_sanitizer() -> OutputSanitizer {
        let registry = Arc::new(RwLock::new(TaintRegistry::new()));
        let classifier = Arc::new(PrivacyClassifier::new(&default_classification_rules()));
        let audit = Arc::new(AuditLog::new(100));
        OutputSanitizer::new(
            registry,
            classifier,
            RedactionStrategy::Remove,
            audit,
            "test-session".to_string(),
        )
    }

    fn make_sanitizer_with_taint(value: &str) -> (OutputSanitizer, Arc<AuditLog>) {
        let registry = Arc::new(RwLock::new(TaintRegistry::new()));
        {
            let mut reg = registry.write().unwrap();
            reg.register(value, "test_rule", SensitivityLevel::HighlySensitive);
        }
        let classifier = Arc::new(PrivacyClassifier::new(&default_classification_rules()));
        let audit = Arc::new(AuditLog::new(100));
        let sanitizer = OutputSanitizer::new(
            registry,
            classifier,
            RedactionStrategy::Remove,
            audit.clone(),
            "test-session".to_string(),
        );
        (sanitizer, audit)
    }

    #[test]
    fn test_sanitize_tainted_data() {
        let (sanitizer, _) = make_sanitizer_with_taint("my-secret-value");
        let result = sanitizer.sanitize_text("The value is my-secret-value here");
        assert!(!result.contains("my-secret-value"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_base64_encoded_taint() {
        let (sanitizer, _) = make_sanitizer_with_taint("secret123");
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "secret123");
        let result = sanitizer.sanitize_text(&format!("Encoded: {}", b64));
        assert!(!result.contains(&b64));
    }

    #[test]
    fn test_sanitize_pii_from_classifier() {
        let sanitizer = make_sanitizer();
        let result = sanitizer.sanitize_text("My SSN is 123-45-6789");
        assert!(!result.contains("123-45-6789"));
    }

    #[test]
    fn test_pass_clean_output() {
        let sanitizer = make_sanitizer();
        let input = "This is a normal response with no sensitive data.";
        let result = sanitizer.sanitize_text(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_audit_log_on_redaction() {
        let (sanitizer, audit) = make_sanitizer_with_taint("secret-data");
        sanitizer.sanitize_text("Contains secret-data here");
        assert!(!audit.is_empty());
        let entries = audit.entries();
        assert_eq!(entries[0].event_type, AuditEventType::OutputRedacted);
    }

    #[test]
    fn test_no_audit_on_clean_output() {
        let (sanitizer, audit) = make_sanitizer_with_taint("secret-data");
        sanitizer.sanitize_text("Nothing sensitive here");
        assert!(audit.is_empty());
    }

    #[test]
    fn test_hook_handler_with_sensitive_response() {
        let (sanitizer, _) = make_sanitizer_with_taint("leaked-secret");
        let event = HookEvent::GenerateEnd(crate::hooks::GenerateEndEvent {
            session_id: "s1".to_string(),
            prompt: "test".to_string(),
            response_text: "Here is leaked-secret in the response".to_string(),
            tool_calls: vec![],
            usage: crate::hooks::TokenUsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
            duration_ms: 100,
        });

        let response = sanitizer.handle(&event);
        assert!(response.modified.is_some());
    }
}
