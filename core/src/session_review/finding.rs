//! Session-scoped review findings bound to a typed [`ReviewSubjectV1`].
//!
//! Lifecycle (host-enforced via these transitions):
//! `pending` → `addressed` (main agent) → `accepted` (reviewer) |
//! `pending` ← `reopen` from `addressed` | `waived` from `pending`.
//!
//! Scenarios tag findings with `scenario_id`; Core does not interpret rubrics.

use super::scenario::SCENARIO_REPLY_TRANSCRIPT;
use super::subject::ReviewSubjectV1;
use super::{
    validate_id, validate_multiline_text, SessionReviewError, SESSION_REVIEW_MAX_TEXT_BYTES,
};
use serde::{Deserialize, Serialize};

pub const SESSION_REVIEW_FINDING_SCHEMA_V1: &str = "a3s.code.session-review-finding.v1";

fn default_scenario_id() -> String {
    SCENARIO_REPLY_TRANSCRIPT.to_owned()
}

/// Durable finding lifecycle for sticky reply / multi-scenario review.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReviewStatusV1 {
    /// Default: inject into the next main-agent turn until addressed or waived.
    Pending,
    /// Main agent claimed this finding; awaiting reviewer acceptance.
    Addressed,
    /// Reviewer accepted the address; terminal.
    Accepted,
    /// Human or host waived; terminal.
    Waived,
}

impl SessionReviewStatusV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Addressed => "addressed",
            Self::Accepted => "accepted",
            Self::Waived => "waived",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Waived)
    }

    /// Sticky prompt injection includes only pending findings.
    pub const fn injects_into_main_prompt(self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// Product-neutral severity for host routing / annotation chrome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReviewSeverityV1 {
    Info,
    Warning,
    Error,
    Blocker,
}

/// Durable pointer into the session transcript (no message body copy).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionReviewAnchorV1 {
    /// Host-stable id for the reviewed assistant turn / bubble.
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_end: Option<u32>,
}

impl SessionReviewAnchorV1 {
    pub fn new(turn_id: impl Into<String>) -> Result<Self, SessionReviewError> {
        let anchor = Self {
            turn_id: turn_id.into(),
            run_id: None,
            char_start: None,
            char_end: None,
        };
        anchor.validate()?;
        Ok(anchor)
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Result<Self, SessionReviewError> {
        let run_id = run_id.into();
        validate_id("anchor.runId", &run_id)?;
        self.run_id = Some(run_id);
        Ok(self)
    }

    pub fn with_span(mut self, start: u32, end: u32) -> Result<Self, SessionReviewError> {
        if end < start {
            return Err(SessionReviewError::InvalidField("anchor.charEnd"));
        }
        self.char_start = Some(start);
        self.char_end = Some(end);
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), SessionReviewError> {
        validate_id("anchor.turnId", &self.turn_id)?;
        if let Some(run_id) = &self.run_id {
            validate_id("anchor.runId", run_id)?;
        }
        match (self.char_start, self.char_end) {
            (None, None) => Ok(()),
            (Some(start), Some(end)) if end >= start => Ok(()),
            (Some(_), Some(_)) => Err(SessionReviewError::InvalidField("anchor.charEnd")),
            (Some(_), None) | (None, Some(_)) => {
                Err(SessionReviewError::InvalidField("anchor.span"))
            }
        }
    }

    /// Strong annotation requires a character span on the turn.
    pub const fn supports_strong_annotation(&self) -> bool {
        self.char_start.is_some() && self.char_end.is_some()
    }
}

/// One durable session review observation (multi-scenario envelope).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionReviewFindingV1 {
    pub schema: String,
    pub finding_id: String,
    pub session_id: String,
    /// Which registered review scenario produced this finding.
    #[serde(default = "default_scenario_id")]
    pub scenario_id: String,
    pub status: SessionReviewStatusV1,
    pub severity: SessionReviewSeverityV1,
    pub category: String,
    pub claim: String,
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub subject: ReviewSubjectV1,
    /// Review job / batch that produced this finding.
    pub source_review_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_by_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_by_review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_reason: Option<String>,
    pub observed_at_ms: u64,
    pub updated_at_ms: u64,
}

impl SessionReviewFindingV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        finding_id: impl Into<String>,
        session_id: impl Into<String>,
        severity: SessionReviewSeverityV1,
        category: impl Into<String>,
        claim: impl Into<String>,
        evidence: impl Into<String>,
        subject: ReviewSubjectV1,
        source_review_id: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, SessionReviewError> {
        let finding = Self {
            schema: SESSION_REVIEW_FINDING_SCHEMA_V1.to_owned(),
            finding_id: finding_id.into(),
            session_id: session_id.into(),
            scenario_id: default_scenario_id(),
            status: SessionReviewStatusV1::Pending,
            severity,
            category: category.into(),
            claim: claim.into(),
            evidence: evidence.into(),
            quote: None,
            suggestion: None,
            subject,
            source_review_id: source_review_id.into(),
            addressed_by_run_id: None,
            accepted_by_review_id: None,
            reopen_reason: None,
            observed_at_ms,
            updated_at_ms: observed_at_ms,
        };
        finding.validate()?;
        Ok(finding)
    }

    /// Convenience for transcript-bound findings (Desktop sticky / science).
    #[allow(clippy::too_many_arguments)]
    pub fn new_transcript(
        finding_id: impl Into<String>,
        session_id: impl Into<String>,
        severity: SessionReviewSeverityV1,
        category: impl Into<String>,
        claim: impl Into<String>,
        evidence: impl Into<String>,
        anchor: SessionReviewAnchorV1,
        source_review_id: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, SessionReviewError> {
        Self::new(
            finding_id,
            session_id,
            severity,
            category,
            claim,
            evidence,
            ReviewSubjectV1::transcript(anchor),
            source_review_id,
            observed_at_ms,
        )
    }

    pub fn with_scenario_id(
        mut self,
        scenario_id: impl Into<String>,
    ) -> Result<Self, SessionReviewError> {
        let scenario_id = scenario_id.into();
        validate_id("scenarioId", &scenario_id)?;
        self.scenario_id = scenario_id;
        Ok(self)
    }

    pub fn with_quote(mut self, quote: impl Into<String>) -> Result<Self, SessionReviewError> {
        let quote = quote.into();
        validate_multiline_text("quote", &quote, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        self.quote = Some(quote);
        Ok(self)
    }

    pub fn with_suggestion(
        mut self,
        suggestion: impl Into<String>,
    ) -> Result<Self, SessionReviewError> {
        let suggestion = suggestion.into();
        validate_multiline_text("suggestion", &suggestion, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        self.suggestion = Some(suggestion);
        Ok(self)
    }

    pub fn transcript_anchor(&self) -> Option<&SessionReviewAnchorV1> {
        self.subject.as_transcript_anchor()
    }

    pub fn validate(&self) -> Result<(), SessionReviewError> {
        if self.schema != SESSION_REVIEW_FINDING_SCHEMA_V1 {
            return Err(SessionReviewError::UnsupportedSchema);
        }
        validate_id("findingId", &self.finding_id)?;
        validate_id("sessionId", &self.session_id)?;
        validate_id("scenarioId", &self.scenario_id)?;
        validate_id("category", &self.category)?;
        validate_multiline_text("claim", &self.claim, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        validate_multiline_text("evidence", &self.evidence, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        if let Some(quote) = &self.quote {
            validate_multiline_text("quote", quote, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        }
        if let Some(suggestion) = &self.suggestion {
            validate_multiline_text("suggestion", suggestion, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        }
        self.subject.validate()?;
        validate_id("sourceReviewId", &self.source_review_id)?;
        if self.observed_at_ms == 0 {
            return Err(SessionReviewError::InvalidField("observedAtMs"));
        }
        if self.updated_at_ms < self.observed_at_ms {
            return Err(SessionReviewError::InvalidField("updatedAtMs"));
        }
        match self.status {
            SessionReviewStatusV1::Pending => {
                if self.addressed_by_run_id.is_some() {
                    return Err(SessionReviewError::InvalidField("addressedByRunId"));
                }
                if self.accepted_by_review_id.is_some() {
                    return Err(SessionReviewError::InvalidField("acceptedByReviewId"));
                }
            }
            SessionReviewStatusV1::Addressed => {
                match &self.addressed_by_run_id {
                    Some(run_id) => validate_id("addressedByRunId", run_id)?,
                    None => return Err(SessionReviewError::InvalidField("addressedByRunId")),
                }
                if self.accepted_by_review_id.is_some() {
                    return Err(SessionReviewError::InvalidField("acceptedByReviewId"));
                }
            }
            SessionReviewStatusV1::Accepted => {
                match &self.addressed_by_run_id {
                    Some(run_id) => validate_id("addressedByRunId", run_id)?,
                    None => return Err(SessionReviewError::InvalidField("addressedByRunId")),
                }
                match &self.accepted_by_review_id {
                    Some(review_id) => validate_id("acceptedByReviewId", review_id)?,
                    None => return Err(SessionReviewError::InvalidField("acceptedByReviewId")),
                }
            }
            SessionReviewStatusV1::Waived => {
                if self.accepted_by_review_id.is_some() {
                    return Err(SessionReviewError::InvalidField("acceptedByReviewId"));
                }
            }
        }
        if let Some(reason) = &self.reopen_reason {
            validate_multiline_text("reopenReason", reason, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        }
        Ok(())
    }

    /// Main agent finished handling this finding.
    pub fn mark_addressed(
        &mut self,
        run_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.validate()?;
        if !matches!(self.status, SessionReviewStatusV1::Pending) {
            return Err(SessionReviewError::InvalidTransition {
                from: self.status.as_str(),
                to: SessionReviewStatusV1::Addressed.as_str(),
            });
        }
        let run_id = run_id.into();
        validate_id("addressedByRunId", &run_id)?;
        if at_ms < self.updated_at_ms {
            return Err(SessionReviewError::InvalidField("updatedAtMs"));
        }
        self.status = SessionReviewStatusV1::Addressed;
        self.addressed_by_run_id = Some(run_id);
        self.accepted_by_review_id = None;
        self.reopen_reason = None;
        self.updated_at_ms = at_ms;
        self.validate()
    }

    /// Reviewer accepted the main-agent address.
    pub fn accept(
        &mut self,
        review_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.validate()?;
        if !matches!(self.status, SessionReviewStatusV1::Addressed) {
            return Err(SessionReviewError::InvalidTransition {
                from: self.status.as_str(),
                to: SessionReviewStatusV1::Accepted.as_str(),
            });
        }
        let review_id = review_id.into();
        validate_id("acceptedByReviewId", &review_id)?;
        if at_ms < self.updated_at_ms {
            return Err(SessionReviewError::InvalidField("updatedAtMs"));
        }
        self.status = SessionReviewStatusV1::Accepted;
        self.accepted_by_review_id = Some(review_id);
        self.reopen_reason = None;
        self.updated_at_ms = at_ms;
        self.validate()
    }

    /// Reviewer rejected the address; return to pending with a reason.
    pub fn reopen(
        &mut self,
        reason: impl Into<String>,
        at_ms: u64,
    ) -> Result<(), SessionReviewError> {
        self.validate()?;
        if !matches!(self.status, SessionReviewStatusV1::Addressed) {
            return Err(SessionReviewError::InvalidTransition {
                from: self.status.as_str(),
                to: SessionReviewStatusV1::Pending.as_str(),
            });
        }
        let reason = reason.into();
        validate_multiline_text("reopenReason", &reason, SESSION_REVIEW_MAX_TEXT_BYTES)?;
        if at_ms < self.updated_at_ms {
            return Err(SessionReviewError::InvalidField("updatedAtMs"));
        }
        self.status = SessionReviewStatusV1::Pending;
        self.addressed_by_run_id = None;
        self.accepted_by_review_id = None;
        self.reopen_reason = Some(reason);
        self.updated_at_ms = at_ms;
        self.validate()
    }

    /// Human / host waived without requiring main-agent address.
    pub fn waive(&mut self, at_ms: u64) -> Result<(), SessionReviewError> {
        self.validate()?;
        if !matches!(self.status, SessionReviewStatusV1::Pending) {
            return Err(SessionReviewError::InvalidTransition {
                from: self.status.as_str(),
                to: SessionReviewStatusV1::Waived.as_str(),
            });
        }
        if at_ms < self.updated_at_ms {
            return Err(SessionReviewError::InvalidField("updatedAtMs"));
        }
        self.status = SessionReviewStatusV1::Waived;
        self.addressed_by_run_id = None;
        self.accepted_by_review_id = None;
        self.reopen_reason = None;
        self.updated_at_ms = at_ms;
        self.validate()
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, SessionReviewError> {
        let finding: Self = super::decode_json_slice(bytes)?;
        finding.validate()?;
        Ok(finding)
    }

    pub fn to_vec(&self) -> Result<Vec<u8>, SessionReviewError> {
        self.validate()?;
        super::encode_json(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_finding() -> SessionReviewFindingV1 {
        let anchor = SessionReviewAnchorV1::new("turn-1")
            .unwrap()
            .with_span(10, 20)
            .unwrap();
        SessionReviewFindingV1::new_transcript(
            "finding-1",
            "session-1",
            SessionReviewSeverityV1::Warning,
            "numeric",
            "Claimed p=0.01 without tool output",
            "No tool record contained a p-value",
            anchor,
            "review-job-1",
            1_000,
        )
        .unwrap()
    }

    #[test]
    fn new_finding_defaults_to_pending_and_injects() {
        let finding = sample_finding();
        assert_eq!(finding.status, SessionReviewStatusV1::Pending);
        assert_eq!(finding.scenario_id, SCENARIO_REPLY_TRANSCRIPT);
        assert!(finding.status.injects_into_main_prompt());
        assert!(finding.subject.supports_strong_annotation());
    }

    #[test]
    fn lifecycle_pending_addressed_accepted() {
        let mut finding = sample_finding();
        finding.mark_addressed("run-2", 1_100).unwrap();
        assert_eq!(finding.status, SessionReviewStatusV1::Addressed);
        assert!(!finding.status.injects_into_main_prompt());
        finding.accept("review-job-2", 1_200).unwrap();
        assert_eq!(finding.status, SessionReviewStatusV1::Accepted);
        assert!(finding.status.is_terminal());
    }

    #[test]
    fn lifecycle_addressed_reopen_returns_pending_with_reason() {
        let mut finding = sample_finding();
        finding.mark_addressed("run-2", 1_100).unwrap();
        finding
            .reopen("p-value still missing from evidence", 1_200)
            .unwrap();
        assert_eq!(finding.status, SessionReviewStatusV1::Pending);
        assert!(finding.status.injects_into_main_prompt());
        assert_eq!(
            finding.reopen_reason.as_deref(),
            Some("p-value still missing from evidence")
        );
        assert!(finding.addressed_by_run_id.is_none());
    }

    #[test]
    fn waive_from_pending_is_terminal() {
        let mut finding = sample_finding();
        finding.waive(1_100).unwrap();
        assert_eq!(finding.status, SessionReviewStatusV1::Waived);
        assert!(finding.status.is_terminal());
        assert!(!finding.status.injects_into_main_prompt());
    }

    #[test]
    fn reject_accept_without_address_and_waive_from_addressed() {
        let mut finding = sample_finding();
        assert!(matches!(
            finding.accept("review-2", 1_100),
            Err(SessionReviewError::InvalidTransition { .. })
        ));
        finding.mark_addressed("run-2", 1_100).unwrap();
        assert!(matches!(
            finding.waive(1_200),
            Err(SessionReviewError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn claim_may_contain_newlines_but_not_nul() {
        let anchor = SessionReviewAnchorV1::new("turn-1").unwrap();
        let ok = SessionReviewFindingV1::new_transcript(
            "finding-2",
            "session-1",
            SessionReviewSeverityV1::Info,
            "citation",
            "line1\nline2",
            "evidence ok",
            anchor.clone(),
            "review-1",
            1,
        );
        assert!(ok.is_ok());
        let bad = SessionReviewFindingV1::new_transcript(
            "finding-3",
            "session-1",
            SessionReviewSeverityV1::Info,
            "citation",
            "bad\0claim",
            "evidence ok",
            anchor,
            "review-1",
            1,
        );
        assert_eq!(bad, Err(SessionReviewError::InvalidField("claim")));
    }

    #[test]
    fn round_trip_wire_helpers() {
        let finding = sample_finding();
        let bytes = finding.to_vec().unwrap();
        let decoded = SessionReviewFindingV1::from_slice(&bytes).unwrap();
        assert_eq!(decoded, finding);
    }

    #[test]
    fn span_requires_both_ends_and_order() {
        let anchor = SessionReviewAnchorV1::new("turn-1").unwrap();
        assert!(anchor.clone().with_span(5, 4).is_err());
        let mut partial = anchor;
        partial.char_start = Some(1);
        assert_eq!(
            partial.validate(),
            Err(SessionReviewError::InvalidField("anchor.span"))
        );
    }
}
