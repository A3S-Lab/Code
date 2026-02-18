//! Security Module
//!
//! Provides a trait-based security interface for A3S Code sessions.
//! External consumers implement `SecurityProvider` to plug in their own
//! security logic (sanitization, taint tracking, injection detection, etc.).

pub mod config;
pub mod default;

pub use config::{RedactionStrategy, SecurityConfig, SensitivityLevel};
pub use default::{DefaultSecurityConfig, DefaultSecurityProvider, SensitivePattern};

use crate::hooks::HookEngine;

/// Trait for pluggable security providers.
///
/// Implement this trait to provide custom security logic for sessions.
/// The default `NoOpSecurityProvider` passes everything through unchanged.
pub trait SecurityProvider: Send + Sync {
    /// Classify and register sensitive data found in input text
    fn taint_input(&self, _text: &str) {}

    /// Sanitize output text by redacting sensitive data.
    /// Returns the sanitized text.
    fn sanitize_output(&self, text: &str) -> String {
        text.to_string()
    }

    /// Securely wipe all session security state
    fn wipe(&self) {}

    /// Register security hooks with the given engine
    fn register_hooks(&self, _hook_engine: &HookEngine) {}

    /// Unregister all hooks from the engine
    fn teardown(&self, _hook_engine: &HookEngine) {}
}

/// No-op security provider (default when security is disabled)
pub struct NoOpSecurityProvider;

impl SecurityProvider for NoOpSecurityProvider {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_provider_passthrough() {
        let provider = NoOpSecurityProvider;
        provider.taint_input("SSN: 123-45-6789");
        let output = provider.sanitize_output("SSN: 123-45-6789");
        assert_eq!(output, "SSN: 123-45-6789");
    }

    #[test]
    fn test_noop_provider_wipe() {
        let provider = NoOpSecurityProvider;
        provider.wipe(); // Should not panic
    }

    #[test]
    fn test_noop_provider_hooks() {
        let engine = HookEngine::new();
        let provider = NoOpSecurityProvider;
        provider.register_hooks(&engine);
        provider.teardown(&engine);
        assert_eq!(engine.hook_count(), 0);
    }
}
