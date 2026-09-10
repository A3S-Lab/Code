//! Pluggable review scenarios — hosts and Use packages supply rubrics here.
//!
//! Code owns fencing, persistence, and lifecycle. Scenarios own evidence
//! collection and evaluation. Core must never `match` on scenario business
//! rules beyond registry lookup.

use super::{
    ReviewSubjectV1, SessionReviewError, SessionReviewFindingV1, SessionReviewSeverityV1,
    SessionReviewStatusV1, SessionReviewStoreV1,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Well-known built-in scenario id for transcript sticky / science reply review.
pub const SCENARIO_REPLY_TRANSCRIPT: &str = "reply.transcript";

/// Host scenario for Desktop / CLI sticky reply review (rubrics stay in the host).
pub struct ReplyTranscriptScenario;

impl ReviewScenario for ReplyTranscriptScenario {
    fn scenario_id(&self) -> &str {
        SCENARIO_REPLY_TRANSCRIPT
    }

    fn triggers(&self) -> ReviewTriggerPolicy {
        ReviewTriggerPolicy::AfterMainSuccess
    }

    fn injects_into_main(&self) -> bool {
        true
    }
}

/// Register the built-in reply.transcript scenario (idempotent replace).
pub fn register_default_scenarios(
    registry: &ReviewScenarioRegistry,
) -> Result<(), SessionReviewError> {
    registry.register(Arc::new(ReplyTranscriptScenario))
}

/// When a scenario may be armed by the host runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewTriggerPolicy {
    /// After a successful main-agent turn (Auto-review).
    AfterMainSuccess,
    /// Periodic / TurnEnd during long work.
    TurnEndPeriodic,
    /// Explicit user or host request only.
    ExplicitRequest,
    /// Scenario is registered but never auto-armed by Code.
    Never,
}

/// Draft finding produced by a scenario before Core assigns session identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewFindingDraft {
    pub finding_id: String,
    pub severity: SessionReviewSeverityV1,
    pub category: String,
    pub claim: String,
    pub evidence: String,
    pub subject: ReviewSubjectV1,
    pub quote: Option<String>,
    pub suggestion: Option<String>,
}

/// Host-supplied review scenario. Rubrics live here — not in Core.
pub trait ReviewScenario: Send + Sync {
    fn scenario_id(&self) -> &str;

    fn triggers(&self) -> ReviewTriggerPolicy {
        ReviewTriggerPolicy::ExplicitRequest
    }

    /// Whether pending findings from this scenario inject into the main prompt.
    fn injects_into_main(&self) -> bool {
        true
    }
}

/// Process-local registry of review scenarios for one session runtime.
#[derive(Default)]
pub struct ReviewScenarioRegistry {
    scenarios: RwLock<HashMap<String, Arc<dyn ReviewScenario>>>,
}

impl ReviewScenarioRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, scenario: Arc<dyn ReviewScenario>) -> Result<(), SessionReviewError> {
        let id = scenario.scenario_id().to_owned();
        super::validate_id("scenarioId", &id)?;
        let mut guard = self
            .scenarios
            .write()
            .map_err(|_| SessionReviewError::Serialization("scenario registry poisoned".into()))?;
        guard.insert(id, scenario);
        Ok(())
    }

    pub fn get(&self, scenario_id: &str) -> Option<Arc<dyn ReviewScenario>> {
        self.scenarios
            .read()
            .ok()
            .and_then(|guard| guard.get(scenario_id).cloned())
    }

    pub fn contains(&self, scenario_id: &str) -> bool {
        self.scenarios
            .read()
            .ok()
            .is_some_and(|guard| guard.contains_key(scenario_id))
    }

    pub fn scenario_ids(&self) -> Vec<String> {
        self.scenarios
            .read()
            .map(|guard| {
                let mut ids: Vec<_> = guard.keys().cloned().collect();
                ids.sort();
                ids
            })
            .unwrap_or_default()
    }
}

/// Admit scenario drafts into the durable session review store.
pub fn admit_finding_drafts(
    store: &mut SessionReviewStoreV1,
    scenario_id: &str,
    source_review_id: &str,
    observed_at_ms: u64,
    drafts: Vec<ReviewFindingDraft>,
) -> Result<usize, SessionReviewError> {
    super::validate_id("scenarioId", scenario_id)?;
    super::validate_id("sourceReviewId", source_review_id)?;
    let mut admitted = 0;
    for draft in drafts {
        let mut finding = SessionReviewFindingV1::new(
            draft.finding_id,
            store.session_id.clone(),
            draft.severity,
            draft.category,
            draft.claim,
            draft.evidence,
            draft.subject,
            source_review_id,
            observed_at_ms,
        )?;
        finding = finding.with_scenario_id(scenario_id)?;
        if let Some(quote) = draft.quote {
            finding = finding.with_quote(quote)?;
        }
        if let Some(suggestion) = draft.suggestion {
            finding = finding.with_suggestion(suggestion)?;
        }
        // New admissions must be pending — enforce via upsert.
        debug_assert!(matches!(finding.status, SessionReviewStatusV1::Pending));
        store.upsert(finding)?;
        admitted += 1;
    }
    Ok(admitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_review::SessionReviewAnchorV1;

    struct DummyScenario(&'static str);

    impl ReviewScenario for DummyScenario {
        fn scenario_id(&self) -> &str {
            self.0
        }

        fn triggers(&self) -> ReviewTriggerPolicy {
            ReviewTriggerPolicy::AfterMainSuccess
        }
    }

    #[test]
    fn registry_registers_and_lists_scenarios() {
        let registry = ReviewScenarioRegistry::new();
        registry
            .register(Arc::new(DummyScenario("reply.transcript")))
            .unwrap();
        registry
            .register(Arc::new(DummyScenario("office.contract")))
            .unwrap();
        assert!(registry.contains("office.contract"));
        assert_eq!(
            registry.scenario_ids(),
            vec!["office.contract".to_owned(), "reply.transcript".to_owned()]
        );
    }

    #[test]
    fn register_default_scenarios_installs_reply_transcript() {
        let registry = ReviewScenarioRegistry::new();
        register_default_scenarios(&registry).unwrap();
        assert!(registry.contains(SCENARIO_REPLY_TRANSCRIPT));
        let scenario = registry.get(SCENARIO_REPLY_TRANSCRIPT).unwrap();
        assert!(scenario.injects_into_main());
        assert_eq!(scenario.triggers(), ReviewTriggerPolicy::AfterMainSuccess);
    }

    #[test]
    fn admit_drafts_tags_scenario_and_stays_pending() {
        let mut store = SessionReviewStoreV1::empty("session-1").unwrap();
        let draft = ReviewFindingDraft {
            finding_id: "f-1".into(),
            severity: SessionReviewSeverityV1::Warning,
            category: "numeric".into(),
            claim: "claim".into(),
            evidence: "evidence".into(),
            subject: ReviewSubjectV1::transcript(SessionReviewAnchorV1::new("turn-1").unwrap()),
            quote: None,
            suggestion: None,
        };
        let n = admit_finding_drafts(
            &mut store,
            "office.contract",
            "review-job-9",
            1_000,
            vec![draft],
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.findings[0].scenario_id, "office.contract");
        assert_eq!(store.findings[0].status, SessionReviewStatusV1::Pending);
        assert_eq!(store.pending_for_scenario("office.contract").len(), 1);
        assert!(store.pending_for_scenario("reply.transcript").is_empty());
    }
}
