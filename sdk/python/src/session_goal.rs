//! Python durable Goal and work-policy methods; Core parses every command.

use super::session::PySession;
use super::*;
use a3s_code_core::GoalTickAdmission as RustGoalTickAdmission;

fn command<T: serde::de::DeserializeOwned>(value: &Bound<'_, PyAny>) -> PyResult<T> {
    serde_json::from_str(&py_any_to_json(value)?)
        .map_err(|error| PyValueError::new_err(format!("invalid goal command: {error}")))
}

fn json_py<T: serde::Serialize>(py: Python<'_>, value: &T) -> PyResult<PyObject> {
    let json = serde_json::to_string(value)
        .map_err(|error| PyRuntimeError::new_err(format!("Serialization error: {error}")))?;
    json_string_to_py(py, &json)
}

#[pymethods]
impl PySession {
    /// Admit a create/revise Goal definition; starts no Run.
    fn apply_goal_definition(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let definition = self
            .inner
            .apply_goal_definition(command(command_json)?)
            .map_err(py_code_error)?;
        json_py(py, &definition)
    }

    /// Propose or explicitly accept a plan; accepts no step result.
    fn apply_goal_plan(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let definition = self
            .inner
            .apply_goal_plan(command(command_json)?)
            .map_err(py_code_error)?;
        json_py(py, &definition)
    }

    /// Record pause/resume/cancel on an idle session.
    fn apply_goal_control(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let definition = self
            .inner
            .apply_goal_control(command(command_json)?)
            .map_err(py_code_error)?;
        json_py(py, &definition)
    }

    /// Pause/cancel even while a goal slice runs: the original RunControl settles it first.
    fn control_goal(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
        grace_ms: u64,
    ) -> PyResult<PyObject> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let definition = py
            .allow_threads(move || {
                get_runtime().block_on(
                    session.control_goal(command, std::time::Duration::from_millis(grace_ms)),
                )
            })
            .map_err(py_code_error)?;
        json_py(py, &definition)
    }

    /// Declare or observe a step wait; returns ``{"recorded", "definition"}``.
    fn apply_goal_wait(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let result = self
            .inner
            .apply_goal_wait(command(command_json)?)
            .map_err(py_code_error)?;
        json_py(py, &result)
    }

    /// The bound Goal read from its original facts, or ``None``.
    fn goal_definition(&self, py: Python<'_>) -> PyResult<PyObject> {
        json_py(py, &self.inner.goal_definition().map_err(py_code_error)?)
    }

    /// Digest of the work policy a Run admitted, or ``None``.
    fn work_policy_digest(&self) -> PyResult<Option<String>> {
        self.inner.work_policy_digest().map_err(py_code_error)
    }

    /// Refuse unless the step and Run are the bound Goal's current ones.
    fn check_goal_step_run(
        &self,
        goal_id: &str,
        goal_revision: u32,
        plan_revision: u32,
        step_id: &str,
        run_id: &str,
    ) -> PyResult<()> {
        self.inner
            .check_goal_step_run(goal_id, goal_revision, plan_revision, step_id, run_id)
            .map_err(py_code_error)
    }

    /// Close stopped goal slices without replay; returns appended fact count.
    fn reconcile_goal_slices(&self) -> PyResult<usize> {
        self.inner.reconcile_goal_slices().map_err(py_code_error)
    }

    /// Inspect a tick decision without reserving or running anything.
    fn preview_goal_tick(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let decision = py
            .allow_threads(move || get_runtime().block_on(session.preview_goal_tick(&command)))
            .map_err(py_code_error)?;
        Ok(decision.as_str().to_owned())
    }

    /// Admit at most one original Run: ``{"decision"}`` or ``{"run": {"snapshot", "replayed"}}``.
    fn tick_goal(&self, py: Python<'_>, command_json: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let admission = py
            .allow_threads(move || get_runtime().block_on(session.tick_goal(&command)))
            .map_err(py_code_error)?;
        match admission {
            RustGoalTickAdmission::Decided(decision) => {
                json_py(py, &serde_json::json!({ "decision": decision.as_str() }))
            }
            RustGoalTickAdmission::Admitted(spawn) => json_py(
                py,
                &serde_json::json!({
                    "run": { "snapshot": spawn.snapshot(), "replayed": spawn.replayed() }
                }),
            ),
        }
    }

    /// `tick_goal` for an interactive host: `{decision}`, `{stream, run}` for a
    /// started slice, or `{replayed}` with the already admitted Run's snapshot.
    fn stream_goal_tick(
        &self,
        py: Python<'_>,
        command_json: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let tick = py
            .allow_threads(move || get_runtime().block_on(session.stream_goal_tick(&command)))
            .map_err(py_code_error)?;
        match tick {
            a3s_code_core::GoalTickStream::Decided(decision) => {
                json_py(py, &serde_json::json!({ "decision": decision.as_str() }))
            }
            a3s_code_core::GoalTickStream::Started(admitted) => {
                super::session_release_contract::run_stream(py, admitted)
            }
            a3s_code_core::GoalTickStream::Replayed(snapshot) => {
                json_py(py, &serde_json::json!({ "replayed": snapshot }))
            }
        }
    }

    /// Admit host-bound review drafts, all or nothing; returns newly admitted count.
    fn admit_session_review_drafts(
        &self,
        scenario_id: &str,
        source_review_id: &str,
        observed_at_ms: u64,
        drafts: &Bound<'_, PyAny>,
    ) -> PyResult<usize> {
        let drafts = serde_json::from_str(&py_any_to_json(drafts)?)
            .map_err(|error| PyValueError::new_err(format!("invalid review drafts: {error}")))?;
        self.inner
            .admit_session_review_drafts(scenario_id, source_review_id, observed_at_ms, drafts)
            .map_err(py_code_error)
    }
}
