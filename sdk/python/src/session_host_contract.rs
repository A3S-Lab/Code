//! Host-facing session review, outcome ledger, and inherited MCP republish.

use super::session::PySession;
use super::*;
use a3s_code_core::outcome_memory::OutcomeKind;
use a3s_code_core::SessionReviewFindingV1;

fn json_py<T: serde::Serialize>(py: Python<'_>, value: &T) -> PyResult<PyObject> {
    let json = serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("Serialization error: {error}")))?;
    json_string_to_py(py, &json)
}

fn parse_outcome(outcome: &str) -> PyResult<OutcomeKind> {
    match outcome {
        "accept" => Ok(OutcomeKind::Accept),
        "revert" => Ok(OutcomeKind::Revert),
        "reject" => Ok(OutcomeKind::Reject),
        other => Err(PyValueError::new_err(format!(
            "outcome must be accept, revert, or reject; got {other}"
        ))),
    }
}

#[pymethods]
impl PySession {
    /// Current durable session-review store.
    fn session_review_store(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_py(py, &self.inner.session_review_store())
    }

    /// Registered scenario ids, sorted.
    fn review_scenario_ids<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.inner.review_scenario_ids())
    }

    /// Pending findings whose registered scenario injects into the next main turn.
    fn pending_session_review_findings(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_py(py, &self.inner.pending_session_review_findings())
    }

    /// Pending findings for one scenario. Empty when that scenario does not inject.
    fn pending_session_review_findings_for_scenario(
        &self,
        py: Python<'_>,
        scenario_id: &str,
    ) -> PyResult<PyObject> {
        json_py(
            py,
            &self
                .inner
                .pending_session_review_findings_for_scenario(scenario_id),
        )
    }

    /// Findings waiting for reviewer acceptance.
    fn addressed_session_review_findings(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_py(py, &self.inner.addressed_session_review_findings())
    }

    /// Insert or replace a finding. New findings must be pending.
    fn upsert_session_review_finding(&self, finding: &Bound<'_, PyDict>) -> PyResult<()> {
        let json = py_dict_to_json(finding)?;
        let finding: SessionReviewFindingV1 = serde_json::from_str(&json).map_err(|error| {
            PyValueError::new_err(format!("Invalid session review finding: {error}"))
        })?;
        self.inner
            .upsert_session_review_finding(finding)
            .map_err(py_code_error)
    }

    /// Mark a pending finding addressed by a main-agent run.
    fn mark_session_review_addressed(
        &self,
        finding_id: &str,
        run_id: &str,
        at_ms: u64,
    ) -> PyResult<()> {
        self.inner
            .mark_session_review_addressed(finding_id, run_id, at_ms)
            .map_err(py_code_error)
    }

    /// Accept an addressed finding.
    fn accept_session_review_finding(
        &self,
        finding_id: &str,
        review_id: &str,
        at_ms: u64,
    ) -> PyResult<()> {
        self.inner
            .accept_session_review_finding(finding_id, review_id, at_ms)
            .map_err(py_code_error)
    }

    /// Reopen an addressed finding.
    fn reopen_session_review_finding(
        &self,
        finding_id: &str,
        reason: &str,
        at_ms: u64,
    ) -> PyResult<()> {
        self.inner
            .reopen_session_review_finding(finding_id, reason, at_ms)
            .map_err(py_code_error)
    }

    /// Waive a pending finding.
    fn waive_session_review_finding(&self, finding_id: &str, at_ms: u64) -> PyResult<()> {
        self.inner
            .waive_session_review_finding(finding_id, at_ms)
            .map_err(py_code_error)
    }

    /// Demote a mistaken product-visible address user prompt to wire-only.
    fn conceal_latest_findings_address_turn(&self) -> usize {
        self.inner.conceal_latest_findings_address_turn()
    }

    /// Record a kept, reverted, or rejected constraint against a change-set digest.
    ///
    /// `outcome` is ``accept``, ``revert``, or ``reject``. A secret-shaped
    /// constraint is not stored. Returns whether the ledger kept the record.
    fn record_outcome(
        &self,
        outcome: &str,
        change_digest: &str,
        constraint: &str,
    ) -> PyResult<bool> {
        self.inner
            .record_outcome(parse_outcome(outcome)?, change_digest, constraint)
            .map_err(py_code_error)
    }

    /// Remember the promoted isolation digest without activating recall.
    fn note_promoted_digest(&self, digest: &str) -> PyResult<bool> {
        self.inner
            .note_promoted_digest(digest)
            .map_err(py_code_error)
    }

    /// Ledger the next turn will serve, including host records since construction.
    fn outcome_ledger_snapshot(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_py(py, &self.inner.outcome_ledger_snapshot())
    }

    /// Rebuild executor tool registrations from inherited MCP managers.
    ///
    /// Call after ``Agent.sync_global_mcp_servers`` so a live session picks up
    /// global connector changes without a restart. Session-local servers stay.
    fn republish_inherited_mcp_tools(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                session
                    .republish_inherited_mcp_tools()
                    .await
                    .map_err(py_code_error)
            })
        })
    }

    /// Whether this session inherits at least one shared MCP manager.
    fn inherits_mcp_managers(&self) -> bool {
        self.inner.inherits_mcp_managers()
    }
}
