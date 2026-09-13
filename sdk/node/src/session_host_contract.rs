//! Host-facing session review, outcome ledger, and inherited MCP republish.
//!
//! Trait-object handles stay Rust-only. These methods cross FFI as the
//! published JSON the core types already serialize.

use super::session::Session;
use super::*;
use a3s_code_core::outcome_memory::OutcomeKind;
use a3s_code_core::SessionReviewFindingV1;

fn json_value<T: serde::Serialize>(value: &T) -> napi::Result<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|error| napi::Error::from_reason(format!("Serialization error: {error}")))
}

fn parse_outcome(outcome: &str) -> napi::Result<OutcomeKind> {
    match outcome {
        "accept" => Ok(OutcomeKind::Accept),
        "revert" => Ok(OutcomeKind::Revert),
        "reject" => Ok(OutcomeKind::Reject),
        other => Err(napi::Error::from_reason(format!(
            "outcome must be accept, revert, or reject; got {other}"
        ))),
    }
}

fn nonnegative_ms(at_ms: i64) -> napi::Result<u64> {
    u64::try_from(at_ms).map_err(|_| napi::Error::from_reason("at_ms must be non-negative"))
}

#[napi]
impl Session {
    /// Current durable session-review store.
    #[napi]
    pub fn session_review_store(&self) -> napi::Result<serde_json::Value> {
        json_value(&self.inner.session_review_store())
    }

    /// Registered scenario ids, sorted.
    #[napi]
    pub fn review_scenario_ids(&self) -> Vec<String> {
        self.inner.review_scenario_ids()
    }

    /// Pending findings whose registered scenario injects into the next main turn.
    #[napi]
    pub fn pending_session_review_findings(&self) -> napi::Result<serde_json::Value> {
        json_value(&self.inner.pending_session_review_findings())
    }

    /// Pending findings for one scenario. Empty when that scenario does not inject.
    #[napi]
    pub fn pending_session_review_findings_for_scenario(
        &self,
        scenario_id: String,
    ) -> napi::Result<serde_json::Value> {
        json_value(
            &self
                .inner
                .pending_session_review_findings_for_scenario(&scenario_id),
        )
    }

    /// Findings waiting for reviewer acceptance.
    #[napi]
    pub fn addressed_session_review_findings(&self) -> napi::Result<serde_json::Value> {
        json_value(&self.inner.addressed_session_review_findings())
    }

    /// Insert or replace a finding. New findings must be pending.
    #[napi(ts_args_type = "finding: any")]
    pub fn upsert_session_review_finding(&self, finding: serde_json::Value) -> napi::Result<()> {
        let finding: SessionReviewFindingV1 = serde_json::from_value(finding).map_err(|error| {
            napi::Error::from_reason(format!("Invalid session review finding: {error}"))
        })?;
        self.inner
            .upsert_session_review_finding(finding)
            .map_err(node_code_error)
    }

    /// Mark a pending finding addressed by a main-agent run.
    #[napi]
    pub fn mark_session_review_addressed(
        &self,
        finding_id: String,
        run_id: String,
        at_ms: i64,
    ) -> napi::Result<()> {
        self.inner
            .mark_session_review_addressed(&finding_id, run_id, nonnegative_ms(at_ms)?)
            .map_err(node_code_error)
    }

    /// Accept an addressed finding.
    #[napi]
    pub fn accept_session_review_finding(
        &self,
        finding_id: String,
        review_id: String,
        at_ms: i64,
    ) -> napi::Result<()> {
        self.inner
            .accept_session_review_finding(&finding_id, review_id, nonnegative_ms(at_ms)?)
            .map_err(node_code_error)
    }

    /// Reopen an addressed finding.
    #[napi]
    pub fn reopen_session_review_finding(
        &self,
        finding_id: String,
        reason: String,
        at_ms: i64,
    ) -> napi::Result<()> {
        self.inner
            .reopen_session_review_finding(&finding_id, reason, nonnegative_ms(at_ms)?)
            .map_err(node_code_error)
    }

    /// Waive a pending finding.
    #[napi]
    pub fn waive_session_review_finding(&self, finding_id: String, at_ms: i64) -> napi::Result<()> {
        self.inner
            .waive_session_review_finding(&finding_id, nonnegative_ms(at_ms)?)
            .map_err(node_code_error)
    }

    /// Demote a mistaken product-visible address user prompt to wire-only.
    #[napi]
    pub fn conceal_latest_findings_address_turn(&self) -> u32 {
        u32::try_from(self.inner.conceal_latest_findings_address_turn()).unwrap_or(u32::MAX)
    }

    /// Record a kept, reverted, or rejected constraint against a change-set digest.
    ///
    /// `outcome` is `accept`, `revert`, or `reject`. A secret-shaped constraint
    /// is not stored. Returns whether the ledger kept the record.
    #[napi]
    pub fn record_outcome(
        &self,
        outcome: String,
        change_digest: String,
        constraint: String,
    ) -> napi::Result<bool> {
        self.inner
            .record_outcome(parse_outcome(&outcome)?, &change_digest, &constraint)
            .map_err(node_code_error)
    }

    /// Remember the promoted isolation digest without activating recall.
    #[napi]
    pub fn note_promoted_digest(&self, digest: String) -> napi::Result<bool> {
        self.inner
            .note_promoted_digest(&digest)
            .map_err(node_code_error)
    }

    /// Ledger the next turn will serve, including host records since construction.
    #[napi]
    pub fn outcome_ledger_snapshot(&self) -> napi::Result<serde_json::Value> {
        json_value(&self.inner.outcome_ledger_snapshot())
    }

    /// Rebuild executor tool registrations from inherited MCP managers.
    ///
    /// Call after `Agent.syncGlobalMcpServers` so a live session picks up
    /// global connector changes without a restart. Session-local servers stay.
    #[napi]
    pub async fn republish_inherited_mcp_tools(&self) -> napi::Result<()> {
        let session = self.inner.clone();
        session
            .republish_inherited_mcp_tools()
            .await
            .map_err(node_code_error)
    }

    /// Whether this session inherits at least one shared MCP manager.
    #[napi]
    pub fn inherits_mcp_managers(&self) -> bool {
        self.inner.inherits_mcp_managers()
    }
}
