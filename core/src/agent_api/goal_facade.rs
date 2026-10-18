//! Typed host definition commands reuse original nonexecuting admission and log resources.

use super::AgentSession;
use crate::error::{CodeError, Result};
use crate::goal_control::{
    DurableGoalDefinitionV1, GoalCommandKindV1, GoalControlCommandV1, GoalDefinitionCommandV1,
    GoalPlanCommandV1, GoalWaitCommandV1,
};
use crate::permissions::PermissionChecker;
use a3s_effect::LogStore;

impl AgentSession {
    /// Serialize a host's legacy goal state access against original Goal admission.
    /// The callback may access host metadata; it cannot start a Run or create a Goal.
    pub async fn with_legacy_goal_state<T, F, Fut>(&self, access: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let lease = self
            .close_handle
            .mutate_immediate(|| self.run_admission.try_acquire(&self.session_id))??;
        let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
        let _writer = log.try_writer_fence().map_err(goal_log_error)?;
        if self.goal_definition()?.is_some() {
            return Err(goal_error(
                "goal.host_driver_conflict: bound Goal excludes legacy state",
            ));
        }
        let value = access().await?;
        self.close_handle.mutate_immediate(|| value)
    }

    /// Admit a create/revise definition without starting a Run or calling a model.
    /// A bound definition does not grant execution; the goal driver has separate admission.
    pub fn apply_goal_definition(
        &self,
        command: GoalDefinitionCommandV1,
    ) -> Result<DurableGoalDefinitionV1> {
        if command.import_source.is_some() {
            return Err(goal_error(
                "goal.conflict: imported definitions require explicit source admission",
            ));
        }
        self.close_handle.mutate_immediate(|| {
            let lease = self.run_admission.try_acquire(&self.session_id)?;
            let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
            let writer = log.try_writer_fence().map_err(goal_log_error)?;
            let thread = crate::fact_control::thread_for_session(&self.session_id);
            let facts = log.read(&thread).map_err(goal_log_error)?;
            let (policy, prepared) = self.prepare_goal_definition(&facts, &thread, &command)?;
            let Some(pending) = prepared.pending else {
                return Ok(prepared.definition);
            };

            self.admit_nonexecuting_standing(log.as_ref(), &facts, &thread)?;
            crate::work_policy::commit_policy(&log, &writer, &thread, &policy)
                .map_err(goal_error)?;
            append_goal(log.as_ref(), &thread, pending, prepared.definition)
        })?
    }

    /// Import through the original owner/writer with a host's read-only source verifier.
    /// The verifier must check original bytes, description and pause posture against
    /// `command.import_source`. It is not exposed to tools and grants no Run authority.
    /// Exact replay needs no new source read. Close or dropping this future forbids commit.
    pub async fn apply_goal_import<F, Fut>(
        &self,
        command: GoalDefinitionCommandV1,
        verify_source: F,
    ) -> Result<DurableGoalDefinitionV1>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        if command.import_source.is_none() {
            return Err(goal_error(
                "goal.conflict: import requires an explicit source",
            ));
        }
        let lease = self
            .close_handle
            .mutate_immediate(|| self.run_admission.try_acquire(&self.session_id))??;
        let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
        let writer = log.try_writer_fence().map_err(goal_log_error)?;
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        let facts = log.read(&thread).map_err(goal_log_error)?;
        let (policy, prepared) = self.prepare_goal_definition(&facts, &thread, &command)?;
        let Some(pending) = prepared.pending else {
            return self.close_handle.mutate_immediate(|| prepared.definition);
        };
        verify_source().await?;
        self.close_handle.mutate_immediate(|| {
            self.admit_nonexecuting_standing(log.as_ref(), &facts, &thread)?;
            crate::work_policy::commit_policy(&log, &writer, &thread, &policy)
                .map_err(goal_error)?;
            append_goal(log.as_ref(), &thread, pending, prepared.definition)
        })?
    }

    fn prepare_goal_definition(
        &self,
        facts: &[a3s_effect::Fact],
        thread: &str,
        command: &GoalDefinitionCommandV1,
    ) -> Result<(
        crate::work_policy::PreparedPolicy,
        crate::goal_control::PreparedDefinition,
    )> {
        self.verify_saved_source(facts)?;
        let policy =
            crate::work_policy::prepare_policy(facts, thread, self.config.work_policy.as_ref())
                .map_err(goal_error)?;
        let digest = policy
            .policy
            .as_ref()
            .map(crate::WorkPolicyV1::digest)
            .transpose()
            .map_err(goal_error)?;
        let prepared = crate::goal_control::prepare_definition(
            facts,
            &self.session_id,
            self.tool_context.workspace_services.workspace_ref(),
            digest.as_deref(),
            command,
        )
        .map_err(goal_error)?;
        Ok((policy, prepared))
    }

    /// Propose or explicitly accept an original plan; no step result is accepted here.
    pub fn apply_goal_plan(&self, command: GoalPlanCommandV1) -> Result<DurableGoalDefinitionV1> {
        self.apply_goal_update(|facts, digest| {
            crate::goal_control::prepare_plan(
                facts,
                &self.session_id,
                self.tool_context.workspace_services.workspace_ref(),
                digest,
                &command,
            )
        })
    }

    /// Cancel a bound definition without deleting history or claiming Run settlement.
    /// An occupied session must first stop through its existing Run control path.
    pub fn apply_goal_control(
        &self,
        command: GoalControlCommandV1,
    ) -> Result<DurableGoalDefinitionV1> {
        self.apply_goal_update(|facts, digest| {
            crate::goal_control::prepare_control(
                facts,
                &self.session_id,
                self.tool_context.workspace_services.workspace_ref(),
                digest,
                &command,
            )
        })
    }

    /// Pause or cancel even while a Run is executing: in a goal-bound session that
    /// Run is a goal slice (possibly not yet recorded), so the original RunControl
    /// cancels and settles it first, then the control is recorded. A control the
    /// goal refuses, or an already recorded one replayed, cancels nothing. If the
    /// slice is recorded between that check and the record, the control conflicts
    /// with the Run already settled; re-read and retry. An old tick then refuses on
    /// its stale sequence. Resume only re-opens the goal; every later tick
    /// re-checks current host bounds.
    pub async fn control_goal(
        &self,
        command: GoalControlCommandV1,
        grace: std::time::Duration,
    ) -> Result<DurableGoalDefinitionV1> {
        if matches!(
            command.kind,
            GoalCommandKindV1::Pause | GoalCommandKindV1::Cancel
        ) {
            let facts = self.goal_facts()?;
            let prepared = crate::goal_control::prepare_control(
                &facts,
                &self.session_id,
                self.tool_context.workspace_services.workspace_ref(),
                self.configured_policy_digest(&facts)?.as_deref(),
                &command,
            )
            .map_err(goal_error)?;
            if prepared.pending.is_none() {
                return Ok(prepared.definition);
            }
            if !self.cancel_and_settle(grace, grace).await {
                return Err(goal_error(
                    "goal.receipt_unsettled: the running goal Run did not settle",
                ));
            }
        }
        self.apply_goal_control(command)
    }

    /// Declare a step wait, or observe one. Due waits read only this host's clock;
    /// a not-due or unchanged observation appends nothing (`recorded=false`).
    /// An observation makes its step ready again; it never accepts progress or starts a Run.
    pub fn apply_goal_wait(
        &self,
        command: GoalWaitCommandV1,
    ) -> Result<crate::goal_control::GoalWaitResultV1> {
        let now = host_clock_unix_ms()?;
        let mut recorded = false;
        let definition = self.apply_goal_update(|facts, digest| {
            let (admitted, prepared) = crate::goal_control::prepare_wait(
                facts,
                &self.session_id,
                self.tool_context.workspace_services.workspace_ref(),
                digest,
                &command,
                now,
            )?;
            recorded = admitted;
            Ok(prepared)
        })?;
        Ok(crate::goal_control::GoalWaitResultV1 {
            recorded,
            definition,
        })
    }

    fn apply_goal_update(
        &self,
        prepare: impl FnOnce(
            &[a3s_effect::Fact],
            Option<&str>,
        ) -> std::result::Result<
            crate::goal_control::PreparedDefinition,
            crate::goal_control::GoalControlError,
        >,
    ) -> Result<DurableGoalDefinitionV1> {
        self.close_handle.mutate_immediate(|| {
            let lease = self.run_admission.try_acquire(&self.session_id)?;
            let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
            let _writer = log.try_writer_fence().map_err(goal_log_error)?;
            let thread = crate::fact_control::thread_for_session(&self.session_id);
            let facts = log.read(&thread).map_err(goal_log_error)?;
            self.verify_saved_source(&facts)?;
            let policy = crate::work_policy::prepare_policy(
                &facts,
                &thread,
                self.config.work_policy.as_ref(),
            )
            .map_err(goal_error)?;
            let digest = policy
                .policy
                .as_ref()
                .map(crate::WorkPolicyV1::digest)
                .transpose()
                .map_err(goal_error)?;
            let prepared = prepare(&facts, digest.as_deref()).map_err(goal_error)?;
            let Some(pending) = prepared.pending else {
                return Ok(prepared.definition);
            };
            self.admit_nonexecuting_standing(log.as_ref(), &facts, &thread)?;
            append_goal(log.as_ref(), &thread, pending, prepared.definition)
        })?
    }

    pub(super) fn admit_nonexecuting_standing(
        &self,
        log: &a3s_effect::FileLog,
        facts: &[a3s_effect::Fact],
        thread: &str,
    ) -> Result<()> {
        let permission = self
            .runtime_permission_policy()
            .or_else(|| self.config.permission_policy.clone())
            .unwrap_or_default();
        let lanes = self
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.yolo_lanes.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        let permission = permission.allow_yolo_lanes(lanes);
        let specs = crate::fact_control::lease_generation_specs(
            crate::fact_control::exposed_tool_specs(&self.tool_executor.definitions(), |name| {
                self.config.permission_checker.as_ref().map_or_else(
                    || permission.expose_to_model(name),
                    |checker| checker.expose_to_model(name),
                )
            }),
            &self.tool_executor,
        );
        let admitted_tools = specs
            .iter()
            .map(|spec| spec.name.clone())
            .collect::<Vec<_>>();
        let harness = self.config.harness.as_ref();
        let config = crate::fact_control::session_harness_config_with(
            self.config.max_tool_rounds,
            1_000_000,
            specs,
            harness.and_then(|options| options.step_limit),
            harness.and_then(|options| options.model_attempts),
        )
        .map_err(goal_error)?;
        crate::standing_context::admit(
            log,
            facts,
            &self.workspace,
            thread,
            None,
            &[],
            &admitted_tools,
            config.budget(),
            config.step_limit(),
            config.model_attempts(),
        )
        .map_err(goal_error)?;
        Ok(())
    }

    /// Read the original definition source; snapshots and summaries cannot replace it.
    pub fn goal_definition(&self) -> Result<Option<DurableGoalDefinitionV1>> {
        let facts = self.goal_facts()?;
        let digest = self.policy_digest_from(&facts)?;
        crate::goal_control::definition_from_facts(
            &facts,
            &self.session_id,
            self.tool_context.workspace_services.workspace_ref(),
            digest.as_deref(),
        )
        .map_err(goal_error)
    }

    /// Digest of the work policy admitted into this session's facts. A session
    /// built with a policy admits it before its first Run.
    pub fn work_policy_digest(&self) -> Result<Option<String>> {
        self.policy_digest_from(&self.goal_facts()?)
    }

    /// Record the configured policy when the session is admitted, through the
    /// same owner and writer a goal definition uses. No policy writes nothing.
    pub(super) fn admit_configured_policy(&self) -> Result<()> {
        let Some(requested) = self.config.work_policy.as_ref() else {
            return Ok(());
        };
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        // Replacement already holds the idle fence; an admitted immutable policy needs no write lease.
        if !crate::work_policy::prepare_policy(&self.goal_facts()?, &thread, Some(requested))
            .map_err(goal_error)?
            .is_pending()
        {
            return Ok(());
        }
        self.close_handle.mutate_immediate(|| {
            let lease = self.run_admission.try_acquire(&self.session_id)?;
            let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
            let writer = log.try_writer_fence().map_err(goal_log_error)?;
            let thread = crate::fact_control::thread_for_session(&self.session_id);
            let facts = log.read(&thread).map_err(goal_log_error)?;
            self.verify_saved_source(&facts)?;
            let policy = crate::work_policy::prepare_policy(&facts, &thread, Some(requested))
                .map_err(goal_error)?;
            if !policy.is_pending() {
                return Ok(());
            }
            self.admit_nonexecuting_standing(log.as_ref(), &facts, &thread)?;
            crate::work_policy::commit_policy(&log, &writer, &thread, &policy).map_err(goal_error)
        })?
    }

    fn policy_digest_from(&self, facts: &[a3s_effect::Fact]) -> Result<Option<String>> {
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        let policy =
            crate::work_policy::prepare_policy(facts, &thread, None).map_err(goal_error)?;
        policy
            .policy
            .as_ref()
            .map(crate::WorkPolicyV1::digest)
            .transpose()
            .map_err(goal_error)
    }

    /// Refuse unless `step_id` is the current step of the bound Goal and
    /// `run_id` is the original Run its current slice admitted. A reference
    /// check only: it grants no authority and accepts no progress.
    pub fn check_goal_step_run(
        &self,
        goal_id: &str,
        goal_revision: u32,
        plan_revision: u32,
        step_id: &str,
        run_id: &str,
    ) -> Result<()> {
        let facts = self.goal_facts()?;
        let digest = self.policy_digest_from(&facts)?;
        let view = crate::goal_control::definition_from_facts(
            &facts,
            &self.session_id,
            self.tool_context.workspace_services.workspace_ref(),
            digest.as_deref(),
        )
        .map_err(goal_error)?
        .ok_or_else(|| goal_error("goal.not_found: no bound Goal"))?;
        if view.binding.goal_id != goal_id
            || view.binding.goal_revision != goal_revision
            || view.plan_revision != Some(plan_revision)
        {
            return Err(goal_error(
                "goal.evidence_stale: Goal or plan revision changed",
            ));
        }
        crate::goal_control::check_step_run(&facts, &view, step_id, run_id).map_err(goal_error)
    }

    /// Close goal slices whose original Run stopped before its owner's closeout:
    /// prepared but never executed, or stopped after its step was accepted.
    /// Nothing runs, replays or is re-accepted, and their usage stays unknown.
    /// An unknown tool outcome refuses (`goal.receipt_unsettled`) until the
    /// original receipt path settles it. A goal whose closure then holds is
    /// recorded completed. Returns how many facts were appended.
    pub fn reconcile_goal_slices(&self) -> Result<usize> {
        self.close_handle.mutate_immediate(|| {
            let lease = self.run_admission.try_acquire(&self.session_id)?;
            let log = lease.nonexecuting_fact_log(&self.run_admission, &self.session_id)?;
            let _writer = log.try_writer_fence().map_err(goal_log_error)?;
            let thread = crate::fact_control::thread_for_session(&self.session_id);
            let facts = log.read(&thread).map_err(goal_log_error)?;
            self.verify_saved_source(&facts)?;
            let digest = self.policy_digest_from(&facts)?;
            let workspace = self.tool_context.workspace_services.workspace_ref();
            let pending = crate::goal_control::reconcile_slices(
                &facts,
                &self.session_id,
                workspace,
                digest.as_deref(),
            )
            .map_err(goal_error)?;
            if !pending.is_empty() {
                log.append(&thread, &pending, None)
                    .map_err(goal_log_error)?;
            }
            let facts = log.read(&thread).map_err(goal_log_error)?;
            let completed = crate::goal_control::prepare_completion(
                &facts,
                &self.session_id,
                workspace,
                digest.as_deref(),
            )
            .map_err(goal_error)?;
            if let Some(completed) = &completed {
                log.append(&thread, std::slice::from_ref(completed), None)
                    .map_err(goal_log_error)?;
            }
            Ok(pending.len() + usize::from(completed.is_some()))
        })?
    }

    /// Inspect a tick without reserving, recovering or executing a Run.
    /// This is a point-in-time view, never execution authority. Without
    /// trusted host `goal_control` bounds a ready step reports `BudgetExhausted`.
    pub async fn preview_goal_tick(
        &self,
        command: &GoalControlCommandV1,
    ) -> Result<crate::goal_control::GoalTickDecisionV1> {
        let source = self.goal_tick_source()?;
        self.preview_goal_tick_with(command, source.as_ref()).await
    }

    /// Admit at most one original Run for an automatic tick, named by its
    /// operation id. The original Run re-validates the same tick, trusted host
    /// bounds and host clock under its writer before any model call.
    pub async fn tick_goal(
        &self,
        command: &GoalControlCommandV1,
    ) -> Result<super::GoalTickAdmission> {
        match self.advancing_goal_tick(command).await? {
            Ok((source, step)) => {
                super::conversation_runtime::spawn_goal_tick(self, command.clone(), source, &step)
                    .await
                    .map(super::GoalTickAdmission::Admitted)
            }
            Err(decision) => Ok(super::GoalTickAdmission::Decided(decision)),
        }
    }

    /// [`Self::tick_goal`] for an interactive host: the started slice's events
    /// (text, tools, confirmations, the terminal) reach the caller as a stream.
    pub async fn stream_goal_tick(
        &self,
        command: &GoalControlCommandV1,
    ) -> Result<super::GoalTickStream> {
        match self.advancing_goal_tick(command).await? {
            Ok((source, step)) => {
                super::conversation_runtime::stream_goal_tick(self, command.clone(), source, &step)
                    .await
            }
            Err(decision) => Ok(super::GoalTickStream::Decided(decision)),
        }
    }

    /// The trusted source and current step of an `Advance` tick, or the decision.
    async fn advancing_goal_tick(
        &self,
        command: &GoalControlCommandV1,
    ) -> Result<
        std::result::Result<
            (crate::goal_control::ContinuationSource, String),
            crate::goal_control::GoalTickDecisionV1,
        >,
    > {
        let source = self.goal_tick_source()?;
        let decision = self
            .preview_goal_tick_with(command, source.as_ref())
            .await?;
        match source {
            Some(source) if decision == crate::goal_control::GoalTickDecisionV1::Advance => {
                // The writer refuses another sequence, so this is the step it binds.
                let step = self
                    .goal_definition()?
                    .filter(|goal| goal.goal_sequence == command.expected_goal_sequence)
                    .and_then(|goal| goal.ready_steps().first().map(|step| step.id.clone()))
                    .ok_or_else(|| {
                        goal_error("goal.conflict: tick source changed before admission")
                    })?;
                Ok(Ok((source, step)))
            }
            _ => Ok(Err(decision)),
        }
    }

    fn goal_tick_source(&self) -> Result<Option<crate::goal_control::ContinuationSource>> {
        let Some(policy) = self.config.goal_control.clone() else {
            return Ok(None);
        };
        Ok(Some(crate::goal_control::ContinuationSource {
            policy,
            observed_unix_ms: host_clock_unix_ms()?,
        }))
    }

    async fn preview_goal_tick_with(
        &self,
        command: &GoalControlCommandV1,
        source: Option<&crate::goal_control::ContinuationSource>,
    ) -> Result<crate::goal_control::GoalTickDecisionV1> {
        if self.is_closed() {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        let runs = self.runs().await;
        let facts = self.goal_facts()?;
        let digest = self.configured_policy_digest(&facts)?;
        crate::goal_control::preview_tick(
            &facts,
            &self.session_id,
            self.tool_context.workspace_services.workspace_ref(),
            digest.as_deref(),
            command,
            self.run_admission.is_active() || runs.iter().any(|run| !run.status.is_terminal()),
            source,
        )
        .map_err(goal_error)
    }

    fn configured_policy_digest(&self, facts: &[a3s_effect::Fact]) -> Result<Option<String>> {
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        crate::work_policy::prepare_policy(facts, &thread, self.config.work_policy.as_ref())
            .map_err(goal_error)?
            .policy
            .as_ref()
            .map(crate::WorkPolicyV1::digest)
            .transpose()
            .map_err(goal_error)
    }

    fn goal_facts(&self) -> Result<Vec<a3s_effect::Fact>> {
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        // Cache the handle the Run lease reuses: a second handle would take the
        // log lock during this read and could refuse a concurrent first Run.
        let log = {
            let slot = self.run_admission.fact_log_slot();
            let mut shared = slot.lock().map_err(|_| {
                goal_error("goal.writer_unavailable: original log slot is poisoned")
            })?;
            match shared.as_ref() {
                Some(log) => Some(std::sync::Arc::clone(log)),
                None => {
                    let directory =
                        crate::session_folder::effect_log_dir(&self.workspace, &self.session_id);
                    if directory.try_exists().map_err(goal_error)? {
                        let log = std::sync::Arc::new(
                            a3s_effect::FileLog::open(directory).map_err(goal_log_error)?,
                        );
                        *shared = Some(std::sync::Arc::clone(&log));
                        Some(log)
                    } else {
                        None
                    }
                }
            }
        };
        let Some(log) = log else {
            self.verify_saved_source(&[])?;
            return Ok(Vec::new());
        };
        let facts = log.read(&thread).map_err(goal_log_error)?;
        self.verify_saved_source(&facts)?;
        Ok(facts)
    }
}

fn append_goal(
    log: &a3s_effect::FileLog,
    thread: &str,
    pending: a3s_effect::NewFact,
    mut view: DurableGoalDefinitionV1,
) -> Result<DurableGoalDefinitionV1> {
    let appended = log
        .append(thread, std::slice::from_ref(&pending), None)
        .map_err(goal_log_error)?;
    view.goal_sequence = appended
        .iter()
        .find(|fact| fact.key == pending.key && fact.kind == pending.kind)
        .map(|fact| fact.seq)
        .ok_or_else(|| goal_error("goal.conflict: committed goal reference is absent"))?;
    if pending.kind == crate::goal_control::GoalFactKindV1::PlanProposed.as_str() {
        view.plan_fact_sequence = Some(view.goal_sequence);
    }
    // Prepared wait references carry sequence 0 until the append assigns one.
    for wait in &mut view.waits {
        if wait.declared_sequence == 0 {
            wait.declared_sequence = view.goal_sequence;
        }
        if let Some(observed) = wait
            .observed
            .as_mut()
            .filter(|observed| observed.sequence == 0)
        {
            observed.sequence = view.goal_sequence;
        }
    }
    Ok(view)
}

fn host_clock_unix_ms() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|now| *now > 0)
        .ok_or_else(|| goal_error("goal.conflict: host clock is unavailable"))
}

fn goal_log_error(error: a3s_effect::ActorError) -> CodeError {
    goal_error(crate::goal_control::GoalControlError::from(error))
}

fn goal_error(error: impl std::fmt::Display) -> CodeError {
    CodeError::Session(error.to_string())
}

#[cfg(test)]
mod tests;
