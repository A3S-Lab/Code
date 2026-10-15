//! Node durable Goal and work-policy methods; Core parses every command.

use super::session::Session;
use super::*;
use a3s_code_core::GoalTickAdmission as RustGoalTickAdmission;

fn command<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> napi::Result<T> {
    serde_json::from_value(value)
        .map_err(|e| napi::Error::from_reason(format!("invalid goal command: {e}")))
}

fn to_json(value: impl serde::Serialize) -> napi::Result<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
}

#[napi]
impl Session {
    /// Admit a create/revise Goal definition; starts no Run.
    #[napi]
    pub fn apply_goal_definition(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let definition = self
            .inner
            .apply_goal_definition(command(command_json)?)
            .map_err(node_code_error)?;
        to_json(definition)
    }

    /// Propose or explicitly accept a plan; accepts no step result.
    #[napi]
    pub fn apply_goal_plan(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let definition = self
            .inner
            .apply_goal_plan(command(command_json)?)
            .map_err(node_code_error)?;
        to_json(definition)
    }

    /// Record pause/resume/cancel on an idle session.
    #[napi]
    pub fn apply_goal_control(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let definition = self
            .inner
            .apply_goal_control(command(command_json)?)
            .map_err(node_code_error)?;
        to_json(definition)
    }

    /// Pause/cancel even while a goal slice runs: the original RunControl settles it first.
    #[napi]
    pub async fn control_goal(
        &self,
        command_json: serde_json::Value,
        grace_ms: u32,
    ) -> napi::Result<serde_json::Value> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let definition = get_runtime()
            .spawn(async move {
                session
                    .control_goal(
                        command,
                        std::time::Duration::from_millis(u64::from(grace_ms)),
                    )
                    .await
            })?
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        to_json(definition)
    }

    /// Declare or observe a step wait; returns `{ recorded, definition }`.
    #[napi]
    pub fn apply_goal_wait(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let result = self
            .inner
            .apply_goal_wait(command(command_json)?)
            .map_err(node_code_error)?;
        to_json(result)
    }

    /// The bound Goal read from its original facts, or null.
    #[napi]
    pub fn goal_definition(&self) -> napi::Result<serde_json::Value> {
        to_json(self.inner.goal_definition().map_err(node_code_error)?)
    }

    /// Digest of the work policy a Run admitted, or null.
    #[napi]
    pub fn work_policy_digest(&self) -> napi::Result<Option<String>> {
        self.inner.work_policy_digest().map_err(node_code_error)
    }

    /// Refuse unless the step and Run are the bound Goal's current ones.
    #[napi]
    pub fn check_goal_step_run(
        &self,
        goal_id: String,
        goal_revision: u32,
        plan_revision: u32,
        step_id: String,
        run_id: String,
    ) -> napi::Result<()> {
        self.inner
            .check_goal_step_run(&goal_id, goal_revision, plan_revision, &step_id, &run_id)
            .map_err(node_code_error)
    }

    /// Close stopped goal slices without replay; returns appended fact count.
    #[napi]
    pub fn reconcile_goal_slices(&self) -> napi::Result<u32> {
        let appended = self
            .inner
            .reconcile_goal_slices()
            .map_err(node_code_error)?;
        u32::try_from(appended).map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    /// Inspect a tick decision without reserving or running anything.
    #[napi]
    pub async fn preview_goal_tick(&self, command_json: serde_json::Value) -> napi::Result<String> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let decision = get_runtime()
            .spawn(async move { session.preview_goal_tick(&command).await })?
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(decision.as_str().to_owned())
    }

    /// Admit at most one original Run: `{ decision }` or `{ run: { snapshot, replayed } }`.
    #[napi]
    pub async fn tick_goal(
        &self,
        command_json: serde_json::Value,
    ) -> napi::Result<serde_json::Value> {
        let command = command(command_json)?;
        let session = self.inner.clone();
        let admission = get_runtime()
            .spawn(async move { session.tick_goal(&command).await })?
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        match admission {
            RustGoalTickAdmission::Decided(decision) => {
                Ok(serde_json::json!({ "decision": decision.as_str() }))
            }
            RustGoalTickAdmission::Admitted(spawn) => {
                let run = agent_run_spawn_to_object(&spawn)?;
                Ok(
                    serde_json::json!({ "run": { "snapshot": run.snapshot, "replayed": run.replayed } }),
                )
            }
        }
    }

    /// Admit host-bound review drafts, all or nothing; returns newly admitted count.
    #[napi]
    pub fn admit_session_review_drafts(
        &self,
        scenario_id: String,
        source_review_id: String,
        observed_at_ms: i64,
        drafts_json: serde_json::Value,
    ) -> napi::Result<u32> {
        let observed_at_ms = super::session_host_contract::nonnegative_ms(observed_at_ms)?;
        let drafts = serde_json::from_value(drafts_json)
            .map_err(|e| napi::Error::from_reason(format!("invalid review drafts: {e}")))?;
        let admitted = self
            .inner
            .admit_session_review_drafts(&scenario_id, &source_review_id, observed_at_ms, drafts)
            .map_err(node_code_error)?;
        u32::try_from(admitted).map_err(|e| napi::Error::from_reason(e.to_string()))
    }
}
