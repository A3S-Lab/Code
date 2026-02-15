//! Security Audit Logging
//!
//! Provides structured audit logging for all security events:
//! taint registration, output redaction, tool blocking, injection detection.

use super::config::SensitivityLevel;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::RwLock;

/// Types of auditable security events
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Sensitive data was registered in the taint registry
    TaintRegistered,
    /// Output was redacted before delivery
    OutputRedacted,
    /// A tool invocation was blocked
    ToolBlocked,
    /// A prompt injection attempt was detected
    InjectionDetected,
    /// Session taint data was securely wiped
    SessionWiped,
}

/// Action taken in response to a security event
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// Operation was allowed to proceed
    Allowed,
    /// Operation was blocked
    Blocked,
    /// Content was redacted
    Redacted,
    /// Event was logged only (no action taken)
    Logged,
}

/// A single audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Session that triggered the event
    pub session_id: String,
    /// Type of security event
    pub event_type: AuditEventType,
    /// Severity level
    pub severity: SensitivityLevel,
    /// Human-readable description
    pub details: String,
    /// Tool name involved (if applicable)
    pub tool_name: Option<String>,
    /// Action taken
    pub action_taken: AuditAction,
}

/// Thread-safe audit log with bounded capacity
pub struct AuditLog {
    entries: RwLock<VecDeque<AuditEntry>>,
    max_entries: usize,
}

impl AuditLog {
    /// Create a new audit log with the given capacity
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(VecDeque::new()),
            max_entries,
        }
    }

    /// Log a new audit entry
    pub fn log(&self, entry: AuditEntry) {
        let Ok(mut entries) = self.entries.write() else {
            tracing::error!("Audit log lock poisoned — dropping audit entry");
            return;
        };
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Get all audit entries
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries
            .read()
            .map(|e| e.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get entries for a specific session
    pub fn entries_for_session(&self, session_id: &str) -> Vec<AuditEntry> {
        self.entries
            .read()
            .map(|e| {
                e.iter()
                    .filter(|e| e.session_id == session_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clear all entries
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    /// Check if the log is empty
    pub fn is_empty(&self) -> bool {
        self.entries.read().map(|e| e.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(session_id: &str, event_type: AuditEventType) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            event_type,
            severity: SensitivityLevel::Sensitive,
            details: "test event".to_string(),
            tool_name: None,
            action_taken: AuditAction::Logged,
        }
    }

    #[test]
    fn test_log_and_retrieve_entries() {
        let log = AuditLog::new(100);
        assert!(log.is_empty());

        log.log(make_entry("s1", AuditEventType::TaintRegistered));
        log.log(make_entry("s1", AuditEventType::OutputRedacted));

        assert_eq!(log.len(), 2);
        let entries = log.entries();
        assert_eq!(entries[0].event_type, AuditEventType::TaintRegistered);
        assert_eq!(entries[1].event_type, AuditEventType::OutputRedacted);
    }

    #[test]
    fn test_filter_by_session() {
        let log = AuditLog::new(100);
        log.log(make_entry("s1", AuditEventType::TaintRegistered));
        log.log(make_entry("s2", AuditEventType::ToolBlocked));
        log.log(make_entry("s1", AuditEventType::OutputRedacted));

        let s1_entries = log.entries_for_session("s1");
        assert_eq!(s1_entries.len(), 2);

        let s2_entries = log.entries_for_session("s2");
        assert_eq!(s2_entries.len(), 1);
        assert_eq!(s2_entries[0].event_type, AuditEventType::ToolBlocked);
    }

    #[test]
    fn test_max_entries_cap() {
        let log = AuditLog::new(3);
        log.log(make_entry("s1", AuditEventType::TaintRegistered));
        log.log(make_entry("s1", AuditEventType::OutputRedacted));
        log.log(make_entry("s1", AuditEventType::ToolBlocked));
        log.log(make_entry("s1", AuditEventType::InjectionDetected));

        assert_eq!(log.len(), 3);
        // First entry should have been evicted
        let entries = log.entries();
        assert_eq!(entries[0].event_type, AuditEventType::OutputRedacted);
        assert_eq!(entries[2].event_type, AuditEventType::InjectionDetected);
    }

    #[test]
    fn test_clear() {
        let log = AuditLog::new(100);
        log.log(make_entry("s1", AuditEventType::TaintRegistered));
        log.log(make_entry("s1", AuditEventType::OutputRedacted));
        assert_eq!(log.len(), 2);

        log.clear();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = make_entry("s1", AuditEventType::ToolBlocked);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("tool_blocked"));
        assert!(json.contains("s1"));

        let parsed: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type, AuditEventType::ToolBlocked);
        assert_eq!(parsed.session_id, "s1");
    }
}
