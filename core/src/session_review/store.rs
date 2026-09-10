//! Bounded session-owned collection of durable review findings.

use super::{
    validate_id, SessionReviewError, SessionReviewFindingV1, SessionReviewStatusV1,
    SESSION_REVIEW_MAX_FINDINGS, SESSION_REVIEW_MAX_MESSAGE_BYTES,
};
use serde::{Deserialize, Serialize};

pub const SESSION_REVIEW_STORE_SCHEMA_V1: &str = "a3s.code.session-review-store.v1";

/// One session's durable review findings (sticky reply / science review).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionReviewStoreV1 {
    pub schema: String,
    pub session_id: String,
    pub findings: Vec<SessionReviewFindingV1>,
}

impl SessionReviewStoreV1 {
    pub fn empty(session_id: impl Into<String>) -> Result<Self, SessionReviewError> {
        let store = Self {
            schema: SESSION_REVIEW_STORE_SCHEMA_V1.to_owned(),
            session_id: session_id.into(),
            findings: Vec::new(),
        };
        store.validate()?;
        Ok(store)
    }

    pub fn validate(&self) -> Result<(), SessionReviewError> {
        if self.schema != SESSION_REVIEW_STORE_SCHEMA_V1 {
            return Err(SessionReviewError::UnsupportedSchema);
        }
        validate_id("sessionId", &self.session_id)?;
        if self.findings.len() > SESSION_REVIEW_MAX_FINDINGS {
            return Err(SessionReviewError::InvalidField("findings"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for finding in &self.findings {
            finding.validate()?;
            if finding.session_id != self.session_id {
                return Err(SessionReviewError::InvalidField("finding.sessionId"));
            }
            if !seen.insert(finding.finding_id.clone()) {
                return Err(SessionReviewError::InvalidField("findingId"));
            }
        }
        Ok(())
    }

    /// Insert or replace by `finding_id`. New findings must be `pending`.
    pub fn upsert(&mut self, finding: SessionReviewFindingV1) -> Result<(), SessionReviewError> {
        finding.validate()?;
        if finding.session_id != self.session_id {
            return Err(SessionReviewError::InvalidField("finding.sessionId"));
        }
        if let Some(existing) = self
            .findings
            .iter()
            .position(|entry| entry.finding_id == finding.finding_id)
        {
            self.findings[existing] = finding;
        } else {
            if self.findings.len() >= SESSION_REVIEW_MAX_FINDINGS {
                return Err(SessionReviewError::InvalidField("findings"));
            }
            if !matches!(finding.status, SessionReviewStatusV1::Pending) {
                return Err(SessionReviewError::InvalidField("finding.status"));
            }
            self.findings.push(finding);
            self.findings
                .sort_unstable_by(|left, right| left.finding_id.cmp(&right.finding_id));
        }
        self.validate()
    }

    pub fn get_mut(
        &mut self,
        finding_id: &str,
    ) -> Result<&mut SessionReviewFindingV1, SessionReviewError> {
        self.findings
            .iter_mut()
            .find(|finding| finding.finding_id == finding_id)
            .ok_or_else(|| SessionReviewError::FindingNotFound(finding_id.to_owned()))
    }

    pub fn mark_addressed(
        &mut self,
        finding_id: &str,
        run_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.get_mut(finding_id)?.mark_addressed(run_id, at_ms)?;
        self.validate()
    }

    pub fn accept(
        &mut self,
        finding_id: &str,
        review_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.get_mut(finding_id)?.accept(review_id, at_ms)?;
        self.validate()
    }

    pub fn reopen(
        &mut self,
        finding_id: &str,
        reason: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.get_mut(finding_id)?.reopen(reason, at_ms)?;
        self.validate()
    }

    pub fn waive(&mut self, finding_id: &str, at_ms: u64) -> Result<(), SessionReviewError> {
        self.get_mut(finding_id)?.waive(at_ms)?;
        self.validate()
    }

    /// Findings that must be injected into the next main-agent user prompt.
    pub fn pending_for_injection(&self) -> Vec<&SessionReviewFindingV1> {
        self.findings
            .iter()
            .filter(|finding| finding.status.injects_into_main_prompt())
            .collect()
    }

    /// Pending findings for one registered scenario (host inject / UI filters).
    pub fn pending_for_scenario(&self, scenario_id: &str) -> Vec<&SessionReviewFindingV1> {
        self.findings
            .iter()
            .filter(|finding| {
                finding.scenario_id == scenario_id && finding.status.injects_into_main_prompt()
            })
            .collect()
    }

    /// Findings waiting for reviewer acceptance after the main agent addressed them.
    pub fn awaiting_acceptance(&self) -> Vec<&SessionReviewFindingV1> {
        self.findings
            .iter()
            .filter(|finding| matches!(finding.status, SessionReviewStatusV1::Addressed))
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending_for_injection().len()
    }

    pub fn addressed_count(&self) -> usize {
        self.awaiting_acceptance().len()
    }

    /// Remap ownership when forking a session snapshot.
    pub fn rebind_session(
        &mut self,
        session_id: impl Into<String>,
    ) -> Result<(), SessionReviewError> {
        let session_id = session_id.into();
        validate_id("sessionId", &session_id)?;
        self.session_id = session_id.clone();
        for finding in &mut self.findings {
            finding.session_id = session_id.clone();
        }
        self.validate()
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, SessionReviewError> {
        let store: Self = super::decode_json_slice(bytes)?;
        store.validate()?;
        Ok(store)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, SessionReviewError> {
        self.validate()?;
        let bytes = super::encode_json(self)?;
        if bytes.len() > SESSION_REVIEW_MAX_MESSAGE_BYTES {
            return Err(SessionReviewError::Encoding);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_review::{SessionReviewAnchorV1, SessionReviewSeverityV1};

    fn pending_finding(id: &str) -> SessionReviewFindingV1 {
        SessionReviewFindingV1::new_transcript(
            id,
            "session-1",
            SessionReviewSeverityV1::Warning,
            "numeric",
            "claim",
            "evidence",
            SessionReviewAnchorV1::new("turn-1").unwrap(),
            "review-1",
            1_000,
        )
        .unwrap()
    }

    #[test]
    fn upsert_and_injection_queues() {
        let mut store = SessionReviewStoreV1::empty("session-1").unwrap();
        store.upsert(pending_finding("f-1")).unwrap();
        store.upsert(pending_finding("f-2")).unwrap();
        assert_eq!(store.pending_count(), 2);
        store.mark_addressed("f-1", "run-9", 1_100).unwrap();
        assert_eq!(store.pending_count(), 1);
        assert_eq!(store.addressed_count(), 1);
        assert_eq!(store.pending_for_injection()[0].finding_id, "f-2");
        store.accept("f-1", "review-2", 1_200).unwrap();
        assert_eq!(store.addressed_count(), 0);
        assert!(!store
            .pending_for_injection()
            .iter()
            .any(|finding| finding.finding_id == "f-1"));
    }

    #[test]
    fn reject_foreign_session_and_duplicate_new_non_pending() {
        let mut store = SessionReviewStoreV1::empty("session-1").unwrap();
        let mut foreign = pending_finding("f-1");
        foreign.session_id = "other".into();
        assert_eq!(
            store.upsert(foreign),
            Err(SessionReviewError::InvalidField("finding.sessionId"))
        );

        let mut addressed = pending_finding("f-2");
        addressed.mark_addressed("run-1", 1_100).unwrap();
        // Direct upsert of a brand-new non-pending finding is rejected.
        let mut store2 = SessionReviewStoreV1::empty("session-1").unwrap();
        assert_eq!(
            store2.upsert(addressed),
            Err(SessionReviewError::InvalidField("finding.status"))
        );
    }

    #[test]
    fn rebind_session_updates_all_findings() {
        let mut store = SessionReviewStoreV1::empty("session-1").unwrap();
        store.upsert(pending_finding("f-1")).unwrap();
        store.rebind_session("session-fork").unwrap();
        assert_eq!(store.session_id, "session-fork");
        assert_eq!(store.findings[0].session_id, "session-fork");
    }

    #[test]
    fn missing_finding_errors() {
        let mut store = SessionReviewStoreV1::empty("session-1").unwrap();
        assert!(matches!(
            store.waive("missing", 1),
            Err(SessionReviewError::FindingNotFound(_))
        ));
    }
}
