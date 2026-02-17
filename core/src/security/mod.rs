//! Security Module
//!
//! Provides security features for A3S Code sessions:
//! - **Output Sanitizer**: Redacts sensitive data from LLM responses
//! - **Taint Tracking**: Tracks sensitive values and their encoded variants
//! - **Tool Interceptor**: Blocks dangerous tool invocations
//! - **Session Isolation**: Per-session security state with secure wipe
//! - **Prompt Injection Defense**: Detects and blocks injection attempts

pub mod audit;
pub mod classifier;
pub mod config;
pub mod injection;
pub mod interceptor;
pub mod sanitizer;
pub mod taint;

pub use audit::{AuditAction, AuditEntry, AuditEventType, AuditLog};
pub use classifier::PrivacyClassifier;
pub use config::{RedactionStrategy, SecurityConfig, SensitivityLevel};
pub use injection::InjectionDetector;
pub use injection::ToolOutputInjectionScanner;
pub use interceptor::ToolInterceptor;
pub use sanitizer::OutputSanitizer;
pub use taint::{TaintId, TaintRegistry};

use crate::hooks::HookEventType;
use crate::hooks::HookHandler;
use crate::hooks::{Hook, HookConfig, HookEngine};
use sanitizer::make_replacement;
use std::sync::{Arc, OnceLock, RwLock};

/// Hook ID prefix for security hooks
const HOOK_PREFIX: &str = "security";

/// Per-session security orchestrator
///
/// Subsystems (`TaintRegistry`, `PrivacyClassifier`, `AuditLog`) are lazily
/// initialized on first access via `OnceLock`. This avoids allocating a
/// `HashMap` (taint), compiling regex (classifier), and creating a 10,000-
/// capacity `Vec` (audit) when the corresponding feature is disabled.
pub struct SecurityGuard {
    session_id: String,
    config: SecurityConfig,
    taint_registry: OnceLock<Arc<RwLock<TaintRegistry>>>,
    classifier: OnceLock<Arc<PrivacyClassifier>>,
    audit_log: OnceLock<Arc<AuditLog>>,
    /// Hook IDs registered by this guard (for teardown)
    hook_ids: std::sync::Mutex<Vec<String>>,
}

impl SecurityGuard {
    /// Create a new SecurityGuard without registering hooks.
    ///
    /// Call [`register_hooks`] separately with a real `HookEngine` to
    /// register security hooks. This avoids the previous bug where hooks
    /// were registered to a temporary engine that was immediately dropped.
    pub fn new(session_id: String, config: SecurityConfig) -> Self {
        Self {
            session_id,
            config,
            taint_registry: OnceLock::new(),
            classifier: OnceLock::new(),
            audit_log: OnceLock::new(),
            hook_ids: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Register security hooks with the given engine.
    ///
    /// Must be called with a long-lived `HookEngine` — not a temporary.
    /// Safe to call multiple times (idempotent: skips if hooks already
    /// registered).
    pub fn register_hooks(&self, hook_engine: &HookEngine) {
        let mut hook_ids = self.hook_ids.lock().unwrap_or_else(|p| p.into_inner());
        if !hook_ids.is_empty() {
            // Already registered
            return;
        }

        // Register tool interceptor hook
        if self.config.features.tool_interceptor {
            let hook_id = format!("{}-interceptor-{}", HOOK_PREFIX, &self.session_id);
            let interceptor = ToolInterceptor::new(
                &self.config,
                self.taint_registry().clone(),
                self.audit_log().clone(),
                self.session_id.clone(),
            );
            hook_engine.register(Hook::new(&hook_id, HookEventType::PreToolUse).with_config(
                HookConfig {
                    priority: 1,
                    ..Default::default()
                },
            ));
            hook_engine.register_handler(&hook_id, Arc::new(interceptor) as Arc<dyn HookHandler>);
            hook_ids.push(hook_id);
        }

        // Register output sanitizer hook
        if self.config.features.output_sanitizer {
            let hook_id = format!("{}-sanitizer-{}", HOOK_PREFIX, &self.session_id);
            let sanitizer = OutputSanitizer::new(
                self.taint_registry().clone(),
                self.classifier().clone(),
                self.config.redaction_strategy,
                self.audit_log().clone(),
                self.session_id.clone(),
            );
            hook_engine.register(Hook::new(&hook_id, HookEventType::GenerateEnd).with_config(
                HookConfig {
                    priority: 1,
                    ..Default::default()
                },
            ));
            hook_engine.register_handler(&hook_id, Arc::new(sanitizer) as Arc<dyn HookHandler>);
            hook_ids.push(hook_id);
        }

        // Register injection detector hook
        if self.config.features.injection_defense {
            let hook_id = format!("{}-injection-{}", HOOK_PREFIX, &self.session_id);
            let detector =
                InjectionDetector::new(self.audit_log().clone(), self.session_id.clone());
            hook_engine.register(
                Hook::new(&hook_id, HookEventType::GenerateStart).with_config(HookConfig {
                    priority: 1,
                    ..Default::default()
                }),
            );
            hook_engine.register_handler(&hook_id, Arc::new(detector) as Arc<dyn HookHandler>);
            hook_ids.push(hook_id);

            // Also register PostToolUse scanner for indirect injection via tool outputs
            let scanner_id = format!("{}-injection-output-{}", HOOK_PREFIX, &self.session_id);
            let scanner =
                ToolOutputInjectionScanner::new(self.audit_log().clone(), self.session_id.clone());
            hook_engine.register(
                Hook::new(&scanner_id, HookEventType::PostToolUse).with_config(HookConfig {
                    priority: 1,
                    ..Default::default()
                }),
            );
            hook_engine.register_handler(&scanner_id, Arc::new(scanner) as Arc<dyn HookHandler>);
            hook_ids.push(scanner_id);
        }
    }

    /// Lazily initialize and return the taint registry
    fn taint_registry(&self) -> &Arc<RwLock<TaintRegistry>> {
        self.taint_registry
            .get_or_init(|| Arc::new(RwLock::new(TaintRegistry::new())))
    }

    /// Lazily initialize and return the privacy classifier
    fn classifier(&self) -> &Arc<PrivacyClassifier> {
        self.classifier
            .get_or_init(|| Arc::new(PrivacyClassifier::new(&self.config.classification_rules)))
    }

    /// Lazily initialize and return the audit log
    fn audit_log(&self) -> &Arc<AuditLog> {
        self.audit_log
            .get_or_init(|| Arc::new(AuditLog::new(10_000)))
    }

    /// Classify input text and register any detected sensitive data as tainted
    pub fn taint_input(&self, text: &str) {
        if !self.config.features.taint_tracking {
            return;
        }

        let result = self.classifier().classify(text);
        if !result.matches.is_empty() {
            let Ok(mut registry) = self.taint_registry().write() else {
                tracing::error!("Taint registry lock poisoned — skipping taint registration");
                return;
            };
            for m in &result.matches {
                let id = registry.register(&m.matched_text, &m.rule_name, m.level);
                self.audit_log().log(AuditEntry {
                    timestamp: chrono::Utc::now(),
                    session_id: self.session_id.clone(),
                    event_type: AuditEventType::TaintRegistered,
                    severity: m.level,
                    details: format!(
                        "Registered tainted value from rule '{}' (id: {})",
                        m.rule_name, id
                    ),
                    tool_name: None,
                    action_taken: AuditAction::Logged,
                });
            }
        }
    }

    /// Sanitize output text by redacting tainted and classified sensitive data
    pub fn sanitize_output(&self, text: &str) -> String {
        if !self.config.features.output_sanitizer {
            return text.to_string();
        }

        let mut result = text.to_string();

        // Check taint registry
        {
            let Ok(registry) = self.taint_registry().read() else {
                tracing::error!("Taint registry lock poisoned — returning unsanitized output");
                return result;
            };
            for (_, entry) in registry.entries_iter() {
                if result.contains(&entry.original_value) {
                    let replacement =
                        make_replacement(&entry.original_value, self.config.redaction_strategy);
                    result = result.replace(&entry.original_value, &replacement);
                }
                for variant in &entry.variants {
                    if result.contains(variant.as_str()) {
                        result = result.replace(variant.as_str(), "[REDACTED]");
                    }
                }
            }
        }

        // Run classifier
        result = self
            .classifier()
            .redact(&result, self.config.redaction_strategy);

        result
    }

    /// Securely wipe all session security state
    pub fn wipe(&self) {
        if let Some(registry) = self.taint_registry.get() {
            if let Ok(mut r) = registry.write() {
                r.wipe();
            } else {
                tracing::error!("Taint registry lock poisoned — cannot wipe");
            }
        }
        if let Some(log) = self.audit_log.get() {
            log.log(AuditEntry {
                timestamp: chrono::Utc::now(),
                session_id: self.session_id.clone(),
                event_type: AuditEventType::SessionWiped,
                severity: SensitivityLevel::Normal,
                details: "Session security state wiped".to_string(),
                tool_name: None,
                action_taken: AuditAction::Logged,
            });
            log.clear();
        }
    }

    /// Unregister all hooks from the engine
    pub fn teardown(&self, hook_engine: &HookEngine) {
        let hook_ids = self.hook_ids.lock().unwrap_or_else(|p| p.into_inner());
        for hook_id in hook_ids.iter() {
            hook_engine.unregister_handler(hook_id);
            hook_engine.unregister(hook_id);
        }
    }

    /// Get audit log entries
    pub fn audit_entries(&self) -> Vec<AuditEntry> {
        self.audit_log().entries()
    }

    /// Get the taint registry (read-only access)
    pub fn get_taint_registry(&self) -> &Arc<RwLock<TaintRegistry>> {
        self.taint_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guard_lifecycle() {
        let engine = HookEngine::new();
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("test-session".to_string(), config);
        guard.register_hooks(&engine);

        // Should have registered 4 hooks (interceptor, sanitizer, injection, output scanner)
        assert_eq!(engine.hook_count(), 4);

        // Taint input with PII
        guard.taint_input("My SSN is 123-45-6789");

        // Verify taint was registered
        {
            let registry = guard.taint_registry().read().unwrap();
            assert!(registry.entry_count() > 0);
        }

        // Sanitize output
        let sanitized = guard.sanitize_output("The SSN 123-45-6789 was found");
        assert!(!sanitized.contains("123-45-6789"));

        // Wipe
        guard.wipe();
        {
            let registry = guard.taint_registry().read().unwrap();
            assert_eq!(registry.entry_count(), 0);
        }

        // Teardown
        guard.teardown(&engine);
        assert_eq!(engine.hook_count(), 0);
    }

    #[test]
    fn test_guard_taint_input_registers_pii() {
        let engine = HookEngine::new();
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("s1".to_string(), config);
        guard.register_hooks(&engine);

        guard.taint_input("Contact me at user@example.com or call 555-123-4567");

        let registry = guard.taint_registry().read().unwrap();
        assert!(registry.entry_count() > 0);

        // Audit should have entries
        let entries = guard.audit_entries();
        assert!(!entries.is_empty());
        assert!(entries
            .iter()
            .any(|e| e.event_type == AuditEventType::TaintRegistered));

        guard.teardown(&engine);
    }

    #[test]
    fn test_guard_sanitize_output() {
        let engine = HookEngine::new();
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("s1".to_string(), config);
        guard.register_hooks(&engine);

        // Register taint
        guard.taint_input("My SSN is 123-45-6789");

        // Output containing the tainted value should be sanitized
        let output = guard.sanitize_output("Found SSN: 123-45-6789 in the data");
        assert!(!output.contains("123-45-6789"));

        guard.teardown(&engine);
    }

    #[test]
    fn test_guard_disabled_features() {
        let engine = HookEngine::new();
        let mut config = SecurityConfig::default();
        config.features.tool_interceptor = false;
        config.features.output_sanitizer = false;
        config.features.injection_defense = false;
        config.features.taint_tracking = false;

        let guard = SecurityGuard::new("s1".to_string(), config);
        guard.register_hooks(&engine);

        // No hooks should be registered
        assert_eq!(engine.hook_count(), 0);

        // Taint input should be a no-op
        guard.taint_input("SSN: 123-45-6789");
        // taint_registry is never initialized when taint_tracking is disabled
        assert!(guard.taint_registry.get().is_none());

        // Sanitize should pass through (output_sanitizer disabled)
        let output = guard.sanitize_output("SSN: 123-45-6789");
        assert_eq!(output, "SSN: 123-45-6789");

        guard.teardown(&engine);
    }

    #[test]
    fn test_guard_wipe_and_teardown() {
        let engine = HookEngine::new();
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("s1".to_string(), config);
        guard.register_hooks(&engine);

        guard.taint_input("SSN: 123-45-6789");
        assert!(guard.taint_registry().read().unwrap().entry_count() > 0);

        guard.wipe();
        assert_eq!(guard.taint_registry().read().unwrap().entry_count(), 0);
        assert!(guard.audit_entries().is_empty());

        guard.teardown(&engine);
        assert_eq!(engine.hook_count(), 0);
    }

    #[test]
    fn test_guard_lazy_init() {
        // Verify subsystems are not initialized until accessed
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("lazy-test".to_string(), config);

        // Before any access, OnceLock should be empty
        assert!(guard.taint_registry.get().is_none());
        assert!(guard.classifier.get().is_none());
        assert!(guard.audit_log.get().is_none());

        // Access triggers initialization
        let _ = guard.taint_registry();
        assert!(guard.taint_registry.get().is_some());
        assert!(guard.classifier.get().is_none()); // still lazy
        assert!(guard.audit_log.get().is_none()); // still lazy
    }

    #[test]
    fn test_register_hooks_idempotent() {
        let engine = HookEngine::new();
        let config = SecurityConfig::default();
        let guard = SecurityGuard::new("s1".to_string(), config);

        guard.register_hooks(&engine);
        let count = engine.hook_count();

        // Second call should be a no-op
        guard.register_hooks(&engine);
        assert_eq!(engine.hook_count(), count);
    }
}
