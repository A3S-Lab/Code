//! Session review finding lifecycle on [`AgentSession`].

use super::AgentSession;
use crate::error::{read_or_recover, write_or_recover, CodeError, Result};
use crate::session_review::{
    ReviewScenario, ReviewScenarioRegistry, SessionReviewFindingV1, SessionReviewStoreV1,
};
use std::sync::Arc;

impl AgentSession {
    /// Current durable session-review store (clone).
    pub fn session_review_store(&self) -> SessionReviewStoreV1 {
        read_or_recover(&self.session_review).clone()
    }

    /// Process-local review scenario registry for this session.
    pub fn review_scenario_registry(&self) -> Arc<ReviewScenarioRegistry> {
        Arc::clone(&self.review_scenarios)
    }

    /// Register a host/Use review scenario (replaces same `scenario_id`).
    pub fn register_review_scenario(&self, scenario: Arc<dyn ReviewScenario>) -> Result<()> {
        self.review_scenarios
            .register(scenario)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    /// Registered scenario ids (sorted).
    pub fn review_scenario_ids(&self) -> Vec<String> {
        self.review_scenarios.scenario_ids()
    }

    /// Findings that must be injected into the next main-agent user prompt.
    ///
    /// Only pending findings whose registered scenario opts into main inject
    /// are returned. Unregistered scenario ids fail closed (no inject).
    pub fn pending_session_review_findings(&self) -> Vec<SessionReviewFindingV1> {
        let registry = &self.review_scenarios;
        read_or_recover(&self.session_review)
            .pending_for_injection()
            .into_iter()
            .filter(|finding| {
                registry
                    .get(&finding.scenario_id)
                    .is_some_and(|scenario| scenario.injects_into_main())
            })
            .cloned()
            .collect()
    }

    /// Pending findings for one scenario (e.g. `reply.transcript`).
    pub fn pending_session_review_findings_for_scenario(
        &self,
        scenario_id: &str,
    ) -> Vec<SessionReviewFindingV1> {
        let injects = self
            .review_scenarios
            .get(scenario_id)
            .is_some_and(|scenario| scenario.injects_into_main());
        if !injects {
            return Vec::new();
        }
        read_or_recover(&self.session_review)
            .pending_for_scenario(scenario_id)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Findings waiting for reviewer acceptance.
    pub fn addressed_session_review_findings(&self) -> Vec<SessionReviewFindingV1> {
        read_or_recover(&self.session_review)
            .awaiting_acceptance()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Insert or replace a finding (new findings must be `pending`).
    pub fn upsert_session_review_finding(&self, finding: SessionReviewFindingV1) -> Result<()> {
        write_or_recover(&self.session_review)
            .upsert(finding)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    pub fn mark_session_review_addressed(
        &self,
        finding_id: &str,
        run_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<()> {
        write_or_recover(&self.session_review)
            .mark_addressed(finding_id, run_id, at_ms)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    pub fn accept_session_review_finding(
        &self,
        finding_id: &str,
        review_id: impl Into<String>,
        at_ms: u64,
    ) -> Result<()> {
        write_or_recover(&self.session_review)
            .accept(finding_id, review_id, at_ms)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    pub fn reopen_session_review_finding(
        &self,
        finding_id: &str,
        reason: impl Into<String>,
        at_ms: u64,
    ) -> Result<()> {
        write_or_recover(&self.session_review)
            .reopen(finding_id, reason, at_ms)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    pub fn waive_session_review_finding(&self, finding_id: &str, at_ms: u64) -> Result<()> {
        write_or_recover(&self.session_review)
            .waive(finding_id, at_ms)
            .map_err(|error| CodeError::Session(error.to_string()))
    }

    /// Mark recent Findings address steering as wire-only if a host path left it
    /// product-visible by mistake.
    ///
    /// Prefer not calling this after a successful address stream: the address
    /// assistant reply should remain a new product message appended to history.
    /// Historical messages must not be cleared. Address **user** steering is
    /// already wire-only via `transcript::is_pure_runtime_steering`.
    pub fn conceal_latest_findings_address_turn(&self) -> usize {
        use crate::llm::TranscriptVisibility;

        let mut history = write_or_recover(&self.history);
        let mut concealed = 0usize;
        let mut idx = history.len();
        while idx > 0 {
            idx -= 1;
            let role = history[idx].role.as_str();
            // Do not conceal assistant address replies — they are the appended
            // product round. Only demote mistaken product-visible address
            // *user* steering prompts.
            if role == "assistant" {
                continue;
            }
            if role == "user" {
                let text = history[idx].text();
                if text.contains("Address each open review finding")
                    || text.contains("open-reply-review-findings")
                {
                    if history[idx].transcript_visibility.is_product() {
                        history[idx].transcript_visibility = TranscriptVisibility::Wire;
                        concealed += 1;
                    }
                }
                break;
            }
        }
        concealed
    }
}
