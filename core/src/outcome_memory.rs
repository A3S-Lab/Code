//! Constraints become Active memory because a change was kept, not because
//! the model mentioned them.
//!
//! Accept binds the change-set digest. Revert and reject stay negative
//! evidence and are absent from Active recall. The binding is the ledger the
//! promoting host writes; restart reloads that file.

use serde::{Deserialize, Serialize};
use std::path::Path;

const SECRET_MARKERS: &[&str] = &[
    "sk-",
    "api_key=",
    "API_KEY=",
    "AKIA",
    "ghp_",
    "BEGIN PRIVATE KEY",
    "Bearer ",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Accept,
    Revert,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub change_digest: String,
    pub constraint: String,
    pub outcome: OutcomeKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeLedger {
    records: Vec<OutcomeRecord>,
    /// Digest of the last promoted isolation change set. Not Active memory
    /// until the host records an accept against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_promoted_digest: Option<String>,
}

impl OutcomeLedger {
    pub fn accept(&mut self, change_digest: &str, constraint: &str) -> bool {
        self.record(change_digest, constraint, OutcomeKind::Accept)
    }

    pub fn revert(&mut self, change_digest: &str, constraint: &str) -> bool {
        self.record(change_digest, constraint, OutcomeKind::Revert)
    }

    pub fn reject(&mut self, change_digest: &str, constraint: &str) -> bool {
        self.record(change_digest, constraint, OutcomeKind::Reject)
    }

    /// Remember the promoted change-set digest. This does not activate recall.
    pub fn note_promoted(&mut self, digest: &str) -> bool {
        if digest.trim().is_empty() {
            return false;
        }
        self.last_promoted_digest = Some(digest.to_string());
        true
    }

    pub fn last_promoted_digest(&self) -> Option<&str> {
        self.last_promoted_digest.as_deref()
    }

    fn record(&mut self, change_digest: &str, constraint: &str, outcome: OutcomeKind) -> bool {
        if change_digest.trim().is_empty() || constraint.trim().is_empty() {
            return false;
        }
        if contains_secret(constraint) {
            return false;
        }
        self.records.retain(|record| {
            !(record.change_digest == change_digest && record.constraint == constraint)
        });
        self.records.push(OutcomeRecord {
            change_digest: change_digest.to_string(),
            constraint: constraint.to_string(),
            outcome,
        });
        true
    }

    pub fn active_recall(&self) -> Vec<&OutcomeRecord> {
        self.records
            .iter()
            .filter(|record| {
                record.outcome == OutcomeKind::Accept && !record.change_digest.is_empty()
            })
            .collect()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec(self).unwrap_or_default())
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }
}

fn contains_secret(text: &str) -> bool {
    SECRET_MARKERS.iter().any(|marker| text.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverted_change_is_absent_from_active_recall() {
        let mut ledger = OutcomeLedger::default();
        assert!(ledger.accept("digest-keep", "prefer explicit errors"));
        assert!(ledger.revert("digest-drop", "do not unwrap"));
        let active = ledger.active_recall();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].constraint, "prefer explicit errors");
        assert!(active
            .iter()
            .all(|record| !record.constraint.contains("unwrap")));
    }

    #[test]
    fn kept_change_is_active_only_with_a_digest() {
        let mut ledger = OutcomeLedger::default();
        assert!(!ledger.accept("", "orphan constraint"));
        assert!(ledger.accept("digest-1", "name the missing evidence"));
        let active = ledger.active_recall();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].change_digest, "digest-1");
    }

    #[test]
    fn restart_retains_the_outcome_binding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outcomes.json");
        let mut ledger = OutcomeLedger::default();
        assert!(ledger.accept("digest-1", "keep the gate"));
        assert!(ledger.revert("digest-2", "dropped idea"));
        ledger.save(&path).unwrap();
        let loaded = OutcomeLedger::load(&path).unwrap();
        assert_eq!(loaded.active_recall().len(), 1);
        assert_eq!(loaded.active_recall()[0].change_digest, "digest-1");
    }

    #[test]
    fn promoted_digest_survives_restart_without_becoming_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outcomes.json");
        let mut ledger = OutcomeLedger::default();
        assert!(ledger.note_promoted("digest-kept"));
        ledger.save(&path).unwrap();
        let loaded = OutcomeLedger::load(&path).unwrap();
        assert_eq!(loaded.last_promoted_digest(), Some("digest-kept"));
        assert!(loaded.active_recall().is_empty());
        let digest = loaded.last_promoted_digest().unwrap().to_string();
        let mut loaded = loaded;
        assert!(loaded.accept(&digest, "name the missing evidence"));
        assert_eq!(loaded.active_recall()[0].change_digest, "digest-kept");
    }

    #[test]
    fn secret_shaped_hunk_is_not_recalled() {
        let mut ledger = OutcomeLedger::default();
        assert!(!ledger.accept("digest-1", "token=sk-live-secret-value"));
        assert!(ledger.active_recall().is_empty());
    }
}
