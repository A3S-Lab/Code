//! Security Taint Tracking
//!
//! Tracks sensitive data values and their encoded variants (base64, hex, URL-encoded)
//! so they can be detected in tool arguments and LLM output.

use super::config::SensitivityLevel;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Unique identifier for a taint entry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaintId(pub Uuid);

impl TaintId {
    /// Generate a new random taint ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaintId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaintId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A tracked piece of sensitive data with its encoded variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaintEntry {
    /// Unique identifier
    pub id: TaintId,
    /// The original sensitive value
    pub original_value: String,
    /// Name of the classification rule that matched
    pub rule_name: String,
    /// Sensitivity level
    pub level: SensitivityLevel,
    /// Encoded variants (base64, hex, url-encoded)
    pub variants: Vec<String>,
    /// When this entry was created
    pub created_at: DateTime<Utc>,
}

/// Registry of tainted (sensitive) data values
pub struct TaintRegistry {
    /// Entries indexed by TaintId
    entries: HashMap<TaintId, TaintEntry>,
    /// Reverse index: value/variant -> TaintId for fast lookup
    value_index: HashMap<String, TaintId>,
    /// Set of all known values for quick contains check
    all_values: HashSet<String>,
}

impl TaintRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            value_index: HashMap::new(),
            all_values: HashSet::new(),
        }
    }

    /// Register a sensitive value and auto-generate encoded variants
    pub fn register(&mut self, value: &str, rule_name: &str, level: SensitivityLevel) -> TaintId {
        // Check if already registered
        if let Some(&id) = self.value_index.get(value) {
            return id;
        }

        let id = TaintId::new();
        let variants = generate_variants(value);

        let entry = TaintEntry {
            id,
            original_value: value.to_string(),
            rule_name: rule_name.to_string(),
            level,
            variants: variants.clone(),
            created_at: Utc::now(),
        };

        // Index the original value
        self.value_index.insert(value.to_string(), id);
        self.all_values.insert(value.to_string());

        // Index all variants
        for variant in &variants {
            self.value_index.insert(variant.clone(), id);
            self.all_values.insert(variant.clone());
        }

        self.entries.insert(id, entry);
        id
    }

    /// Check if a text contains any tainted value (exact match against all variants)
    pub fn contains(&self, text: &str) -> bool {
        for value in &self.all_values {
            if text.contains(value.as_str()) {
                return true;
            }
        }
        false
    }

    /// Check for encoded variants in text by decoding base64/hex/url segments
    pub fn check_encoded(&self, text: &str) -> bool {
        // Try to decode base64 segments
        for word in text.split_whitespace() {
            // Try base64 decode
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(word) {
                if let Ok(decoded_str) = String::from_utf8(decoded) {
                    if self.contains_original(&decoded_str) {
                        return true;
                    }
                }
            }

            // Try URL decode
            if word.contains('%') {
                if let Ok(decoded) = urldecode(word) {
                    if self.contains_original(&decoded) {
                        return true;
                    }
                }
            }

            // Try hex decode
            if word.len() >= 4 && word.len() % 2 == 0 && word.chars().all(|c| c.is_ascii_hexdigit())
            {
                if let Some(decoded) = hex_decode(word) {
                    if self.contains_original(&decoded) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if text contains any original (non-variant) tainted value
    fn contains_original(&self, text: &str) -> bool {
        for entry in self.entries.values() {
            if text.contains(&entry.original_value) {
                return true;
            }
        }
        false
    }

    /// Securely wipe all taint data
    pub fn wipe(&mut self) {
        self.entries.clear();
        self.value_index.clear();
        self.all_values.clear();
    }

    /// Get the number of tracked entries
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get a taint entry by ID
    pub fn get(&self, id: &TaintId) -> Option<&TaintEntry> {
        self.entries.get(id)
    }

    /// Find the taint ID for a given value or variant
    pub fn lookup(&self, value: &str) -> Option<TaintId> {
        self.value_index.get(value).copied()
    }

    /// Iterate over all entries
    pub fn entries_iter(&self) -> impl Iterator<Item = (&TaintId, &TaintEntry)> {
        self.entries.iter()
    }
}

impl Default for TaintRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate encoded variants of a sensitive value
fn generate_variants(value: &str) -> Vec<String> {
    let mut variants = Vec::new();

    // Base64 encoded
    let b64 = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    variants.push(b64);

    // Hex encoded
    let hex: String = value.bytes().map(|b| format!("{:02x}", b)).collect();
    variants.push(hex);

    // URL encoded
    let url_encoded = urlencode(value);
    if url_encoded != value {
        variants.push(url_encoded);
    }

    variants
}

/// Simple URL encoding
fn urlencode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

/// Simple URL decoding
fn urldecode(s: &str) -> Result<String, ()> {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &s[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).map_err(|_| ())
}

/// Decode hex string to UTF-8
fn hex_decode(s: &str) -> Option<String> {
    let bytes: Result<Vec<u8>, _> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect();
    bytes.ok().and_then(|b| String::from_utf8(b).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_contains() {
        let mut registry = TaintRegistry::new();
        registry.register(
            "4111-1111-1111-1111",
            "credit_card",
            SensitivityLevel::HighlySensitive,
        );

        assert!(registry.contains("my card is 4111-1111-1111-1111 ok"));
        assert!(!registry.contains("no sensitive data here"));
        assert_eq!(registry.entry_count(), 1);
    }

    #[test]
    fn test_variant_generation_base64() {
        let mut registry = TaintRegistry::new();
        registry.register("secret123", "api_key", SensitivityLevel::HighlySensitive);

        let b64 = base64::engine::general_purpose::STANDARD.encode("secret123");
        assert!(registry.contains(&b64));
    }

    #[test]
    fn test_variant_generation_hex() {
        let mut registry = TaintRegistry::new();
        registry.register("abc", "test", SensitivityLevel::Sensitive);

        // "abc" in hex is "616263"
        assert!(registry.contains("616263"));
    }

    #[test]
    fn test_check_encoded_base64() {
        let mut registry = TaintRegistry::new();
        registry.register("sensitive-data", "test", SensitivityLevel::Sensitive);

        let b64 = base64::engine::general_purpose::STANDARD.encode("sensitive-data");
        assert!(registry.check_encoded(&format!("here is {}", b64)));
    }

    #[test]
    fn test_check_encoded_hex() {
        let mut registry = TaintRegistry::new();
        registry.register("abc", "test", SensitivityLevel::Sensitive);

        // "abc" in hex
        assert!(registry.check_encoded("decoded hex: 616263"));
    }

    #[test]
    fn test_duplicate_register_returns_same_id() {
        let mut registry = TaintRegistry::new();
        let id1 = registry.register("value1", "rule1", SensitivityLevel::Sensitive);
        let id2 = registry.register("value1", "rule1", SensitivityLevel::Sensitive);
        assert_eq!(id1, id2);
        assert_eq!(registry.entry_count(), 1);
    }

    #[test]
    fn test_wipe_clears_all() {
        let mut registry = TaintRegistry::new();
        registry.register("value1", "rule1", SensitivityLevel::Sensitive);
        registry.register("value2", "rule2", SensitivityLevel::HighlySensitive);
        assert_eq!(registry.entry_count(), 2);

        registry.wipe();
        assert_eq!(registry.entry_count(), 0);
        assert!(!registry.contains("value1"));
        assert!(!registry.contains("value2"));
    }

    #[test]
    fn test_lookup() {
        let mut registry = TaintRegistry::new();
        let id = registry.register("test-value", "rule1", SensitivityLevel::Sensitive);

        assert_eq!(registry.lookup("test-value"), Some(id));
        assert!(registry.lookup("nonexistent").is_none());
    }

    #[test]
    fn test_get_entry() {
        let mut registry = TaintRegistry::new();
        let id = registry.register("test-value", "rule1", SensitivityLevel::Sensitive);

        let entry = registry.get(&id).unwrap();
        assert_eq!(entry.original_value, "test-value");
        assert_eq!(entry.rule_name, "rule1");
        assert_eq!(entry.level, SensitivityLevel::Sensitive);
        assert!(!entry.variants.is_empty());
    }

    #[test]
    fn test_url_encoding_variant() {
        let mut registry = TaintRegistry::new();
        registry.register("hello world", "test", SensitivityLevel::Sensitive);

        // URL-encoded "hello world" = "hello%20world"
        assert!(registry.contains("hello%20world"));
    }

    #[test]
    fn test_taint_id_default() {
        let id1 = TaintId::default();
        let id2 = TaintId::default();
        // Each default creates a new unique ID
        assert_ne!(id1.0, id2.0);
    }

    #[test]
    fn test_taint_id_display() {
        let id = TaintId::new();
        let display = format!("{}", id);
        assert!(!display.is_empty());
        // UUID format
        assert_eq!(display.len(), 36);
    }

    #[test]
    fn test_taint_registry_default() {
        let registry = TaintRegistry::default();
        assert!(registry.lookup("anything").is_none());
    }

    #[test]
    fn test_urldecode_valid() {
        let decoded = urldecode("hello%20world");
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), "hello world");
    }

    #[test]
    fn test_urldecode_no_encoding() {
        let decoded = urldecode("hello");
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), "hello");
    }

    #[test]
    fn test_urldecode_invalid_hex_passthrough() {
        // Invalid hex after % just passes through as raw bytes
        let decoded = urldecode("hello%ZZworld");
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), "hello%ZZworld");
    }

    #[test]
    fn test_urlencode_special_chars() {
        let encoded = urlencode("a b@c");
        assert_eq!(encoded, "a%20b%40c");
    }

    #[test]
    fn test_urlencode_no_special() {
        let encoded = urlencode("hello");
        assert_eq!(encoded, "hello");
    }

    #[test]
    fn test_generate_variants() {
        let variants = generate_variants("test");
        // Should have base64 and hex (no url-encoded since "test" has no special chars)
        assert!(variants.len() >= 2);
        // Base64 of "test" = "dGVzdA=="
        assert!(variants.contains(&"dGVzdA==".to_string()));
        // Hex of "test" = "74657374"
        assert!(variants.contains(&"74657374".to_string()));
    }

    #[test]
    fn test_generate_variants_with_special_chars() {
        let variants = generate_variants("hello world");
        // Should have base64, hex, and url-encoded
        assert!(variants.len() >= 3);
        assert!(variants.contains(&"hello%20world".to_string()));
    }

    #[test]
    fn test_entries_iter() {
        let mut registry = TaintRegistry::new();
        registry.register("val1", "rule1", SensitivityLevel::Sensitive);
        registry.register("val2", "rule2", SensitivityLevel::HighlySensitive);

        let entries: Vec<_> = registry.entries_iter().collect();
        assert_eq!(entries.len(), 2);
    }
}
