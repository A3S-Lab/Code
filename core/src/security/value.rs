//! Typed provenance for values crossing runtime security boundaries.

use super::SecurityProvider;
use serde::{Deserialize, Serialize};

/// Trust provenance assigned by the owning adapter, never inferred from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Untrusted,
    Derived,
    Trusted,
}

/// Taint classification is independent of instruction authority and redaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaintLabel {
    Unknown,
    Sensitive,
    Secret,
    PromptInjection,
}

/// Whether the configured provider has processed the complete value.
///
/// `Applied` is not a guarantee that the value is public or safe to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizationState {
    Pending,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityLabel {
    pub trust: TrustLevel,
    pub taint: TaintLabel,
    pub sanitization: SanitizationState,
}

impl SecurityLabel {
    pub const fn untrusted() -> Self {
        Self {
            trust: TrustLevel::Untrusted,
            taint: TaintLabel::Unknown,
            sanitization: SanitizationState::Pending,
        }
    }

    pub const fn trusted() -> Self {
        Self {
            trust: TrustLevel::Trusted,
            ..Self::untrusted()
        }
    }

    pub const fn derived(taint: TaintLabel) -> Self {
        Self {
            trust: TrustLevel::Derived,
            taint,
            sanitization: SanitizationState::Pending,
        }
    }

    fn sanitized(self) -> Self {
        Self {
            sanitization: SanitizationState::Applied,
            ..self
        }
    }
}

/// A label travels with its value until an explicit boundary consumes it.
/// These labels describe provenance; they never grant execution permission.
#[must_use]
#[derive(Clone, PartialEq, Eq)]
pub struct TaintedValue<T> {
    value: T,
    label: SecurityLabel,
}

impl<T> std::fmt::Debug for TaintedValue<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaintedValue")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl<T> TaintedValue<T> {
    pub fn new(value: T, label: SecurityLabel) -> Self {
        Self { value, label }
    }

    pub fn untrusted(value: T) -> Self {
        Self::new(value, SecurityLabel::untrusted())
    }

    pub fn trusted(value: T) -> Self {
        Self::new(value, SecurityLabel::trusted())
    }

    pub fn label(&self) -> SecurityLabel {
        self.label
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_parts(self) -> (T, SecurityLabel) {
        (self.value, self.label)
    }
}

/// Apply output sanitization without promoting trust or declassifying taint.
pub fn sanitize_tainted_text(
    provider: &dyn SecurityProvider,
    value: TaintedValue<String>,
) -> TaintedValue<String> {
    let (value, label) = value.into_parts();
    TaintedValue::new(provider.sanitize_output(&value), label.sanitized())
}

/// Convenience adapter for text values that enter the egress boundary without
/// an existing wrapper.
pub fn sanitize_text(provider: &dyn SecurityProvider, value: &str) -> String {
    sanitize_tainted_text(provider, TaintedValue::untrusted(value.to_owned()))
        .into_parts()
        .0
}

/// Process complete JSON string values while retaining keys and protocol shape.
/// Keys are protocol field names, not a channel for arbitrary output text.
pub fn sanitize_tainted_json(
    provider: &dyn SecurityProvider,
    value: TaintedValue<serde_json::Value>,
) -> TaintedValue<serde_json::Value> {
    fn sanitize(provider: &dyn SecurityProvider, value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(value) => {
                serde_json::Value::String(provider.sanitize_output(&value))
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .into_iter()
                    .map(|value| sanitize(provider, value))
                    .collect(),
            ),
            serde_json::Value::Object(values) => serde_json::Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, sanitize(provider, value)))
                    .collect(),
            ),
            value => value,
        }
    }

    let (value, label) = value.into_parts();
    TaintedValue::new(sanitize(provider, value), label.sanitized())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::DefaultSecurityProvider;

    #[test]
    fn typed_value_boundary_preserves_provenance_and_redacts_nested_json() {
        let provider = DefaultSecurityProvider::new();
        let value = TaintedValue::untrusted(serde_json::json!({
            "email": "user@example.com",
            "nested": ["123-45-6789"],
        }));
        assert_eq!(value.label(), SecurityLabel::untrusted());

        let (sanitized, label) = sanitize_tainted_json(&provider, value).into_parts();
        assert_eq!(label.trust, TrustLevel::Untrusted);
        assert_eq!(label.taint, TaintLabel::Unknown);
        assert_eq!(label.sanitization, SanitizationState::Applied);
        assert_eq!(sanitized["email"], "[REDACTED:EMAIL]");
        assert_eq!(sanitized["nested"][0], "[REDACTED:SSN]");
    }

    #[test]
    fn redaction_does_not_declassify_secrets_or_grant_instruction_authority() {
        let value = TaintedValue::new(
            "user@example.com".to_string(),
            SecurityLabel {
                taint: TaintLabel::Secret,
                ..SecurityLabel::untrusted()
            },
        );
        let sanitized = sanitize_tainted_text(&DefaultSecurityProvider::new(), value);
        assert_eq!(sanitized.label().trust, TrustLevel::Untrusted);
        assert_eq!(sanitized.label().taint, TaintLabel::Secret);
        assert_eq!(sanitized.label().sanitization, SanitizationState::Applied);
        assert_eq!(sanitized.value(), "[REDACTED:EMAIL]");
    }

    #[test]
    fn value_debug_never_exposes_content() {
        let value = TaintedValue::trusted("private-canary".to_string());
        assert_eq!(value.label(), SecurityLabel::trusted());
        assert!(!format!("{value:?}").contains("private-canary"));
    }
}
