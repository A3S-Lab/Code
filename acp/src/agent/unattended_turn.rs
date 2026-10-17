//! The unattended prompt: the host admission loop around repeated core
//! streams of one ACP prompt. Interactive prompts never reach this module.

use super::*;
use crate::unattended::result::{
    classify_failure, persist_result, ExecutionPhase, RunProgress, TerminalReason,
};
use crate::unattended::{
    bind_continuation, first_prompt, host_admission_step, host_verification_report,
    non_terminal_continuation, note_event, AttemptFacts, HostAdmissionBudget, HostAdmissionStep,
    Unattended, BEFORE_ANY_GATE_CONTINUATION, FRESH_REVIEW_FOCUS, IDLE_AFTER_BIND_CONTINUATION,
    UNPROVEN_STOP_CONTINUATION,
};
use a3s_code_core::execution_identity::ExecutionResultOutcomeV1;
use anyhow::{bail, Context};

enum Attempt {
    Ended {
        turn_permit: OwnedMutexGuard<()>,
        facts: AttemptFacts,
        join_failure: Option<String>,
    },
    Cancelled(acp::Result<acp::PromptResponse>),
}

impl A3sCodeAgent {
    /// Run `task` to a terminal host decision, then write the result record.
    /// The turn permit is held across attempts, so no other prompt interleaves.
    pub(super) async fn unattended_prompt(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        task: &str,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        unattended: &Unattended,
    ) -> acp::Result<acp::PromptResponse> {
        let mut progress = RunProgress::new();
        let run = self
            .unattended_run(
                session_id,
                session,
                task,
                generation,
                turn_permit,
                unattended,
                &mut progress,
            )
            .await;
        let mut failure = match run {
            Ok(Some(cancelled)) => return cancelled,
            Ok(None) => None,
            Err(error) => Some(error),
        };
        if let Some(error) = &failure {
            progress.remember_failure(error);
        }
        if failure.is_none() && !progress.has_action_evidence() {
            progress.mark_evidence_missing();
            failure = Some(anyhow::anyhow!(
                "A3S Code agent ended without a successful tool execution"
            ));
        }
        let (outcome, reason) = match &failure {
            None => (
                ExecutionResultOutcomeV1::Succeeded,
                TerminalReason::AgentEnd,
            ),
            Some(error) => classify_failure(&progress, error),
        };
        if let Some(path) = &unattended.result_file {
            if let Err(error) = persist_result(path, &progress.report(outcome, reason)).await {
                tracing::error!(path = %path.display(), "could not write the result record: {error:#}");
                failure.get_or_insert(error);
            }
        }
        match failure {
            None => Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
            // These outcomes are recorded in the result file. Returning a
            // JSON-RPC internal error makes Harbor's summary say only
            // "Internal error" and hides the reason.
            Some(_)
                if matches!(
                    reason,
                    TerminalReason::ProviderRejected
                        | TerminalReason::WorkerExitedWithoutTerminalEvent
                        | TerminalReason::StreamClosedWithoutTerminalEvent
                        | TerminalReason::DeadlineExceeded
                        | TerminalReason::EvidenceMissing
                ) =>
            {
                Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
            }
            Some(error) => Err(agent_failure_error(&format!("{error:#}"))),
        }
    }

    /// `Ok(Some(_))` is a cancelled prompt and writes no record, like a
    /// killed native runner.
    #[allow(clippy::too_many_arguments)]
    async fn unattended_run(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        task: &str,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        unattended: &Unattended,
        progress: &mut RunProgress,
    ) -> anyhow::Result<Option<acp::Result<acp::PromptResponse>>> {
        let mut prompt = first_prompt(task);
        let mut bound_digests = HashSet::new();
        let mut budget = HostAdmissionBudget::default();
        let mut turn_permit = turn_permit;
        let mut goal_tick = if unattended.goal_continuation {
            Some(crate::goal::bind_task_goal(session, task).context("bind the task goal")?)
        } else {
            None
        };
        loop {
            if progress.duration_ms() >= unattended.max_execution_ms {
                bail!("A3S Code execution deadline exceeded");
            }
            progress.turns += 1;
            progress.phase = ExecutionPhase::Streaming;
            progress.terminal_event = false;
            // Each attempt is a fresh core run with the full core budget; the
            // run-wide deadline is what remains of the host budget.
            let remaining = std::time::Duration::from_millis(
                unattended
                    .max_execution_ms
                    .saturating_sub(progress.duration_ms()),
            );
            let stream = match goal_tick.take() {
                Some(tick) => match session
                    .stream_goal_tick(&tick)
                    .await
                    .context("start the goal slice")?
                {
                    a3s_code_core::GoalTickStream::Started(stream) => stream,
                    a3s_code_core::GoalTickStream::Decided(decision) => {
                        bail!("the bound task goal did not advance: {}", decision.as_str())
                    }
                    a3s_code_core::GoalTickStream::Replayed(snapshot) => {
                        bail!("the goal slice replayed Run {}", snapshot.id)
                    }
                },
                None => session
                    .stream_with_run(&prompt, None)
                    .await
                    .context("start A3S Code stream")?,
            };
            let attempt = self
                .unattended_attempt(
                    session_id,
                    session,
                    stream,
                    generation,
                    turn_permit,
                    progress,
                    remaining,
                )
                .await?;
            let (permit, facts, join_failure) = match attempt {
                Attempt::Cancelled(response) => return Ok(Some(response)),
                Attempt::Ended {
                    turn_permit,
                    facts,
                    join_failure,
                } => (turn_permit, facts, join_failure),
            };
            turn_permit = permit;
            if unattended.goal_continuation {
                if facts
                    .last_error
                    .as_deref()
                    .is_some_and(crate::unattended::result::is_provider_rejection)
                    || progress
                        .last_error()
                        .is_some_and(crate::unattended::result::is_provider_rejection)
                {
                    let detail = facts
                        .last_error
                        .clone()
                        .or_else(|| progress.last_error().map(str::to_owned))
                        .unwrap_or_else(|| "provider rejected the request".into());
                    bail!("{detail}");
                }
                if let Some(failure) = join_failure {
                    bail!("join A3S Code stream: {failure}");
                }
                // The next slice is another original Run. A freeform prompt
                // here would leave the goal unfinished and settle it as
                // interrupted.
                match crate::goal::bound_goal_step(session)
                    .await
                    .context("next goal slice")?
                {
                    crate::goal::BoundGoalStep::Advance(tick) => {
                        goal_tick = Some(tick);
                        continue;
                    }
                    crate::goal::BoundGoalStep::Finished => return Ok(None),
                    crate::goal::BoundGoalStep::Stopped(reason) => {
                        bail!("the bound task goal stopped before it was finished: {reason}")
                    }
                }
            }
            let step = host_admission_step(
                &mut bound_digests,
                &mut budget,
                &facts,
                progress.terminal_event,
            );
            tracing::info!(
                ?step,
                bound = bound_digests.len(),
                "unattended host admission"
            );
            match step {
                HostAdmissionStep::BindNewDigest => {
                    let digest = facts
                        .gate_digest
                        .context("host admission bind missing mutation digest")?;
                    session.record_verification_reports([host_verification_report(&digest)]);
                    prompt = bind_continuation(&digest);
                }
                HostAdmissionStep::SameDigestStillOpen => {
                    let digest = facts.gate_digest.unwrap_or_default();
                    bail!("completion gate still open after host verification for {digest}");
                }
                HostAdmissionStep::NudgeIdleAfterBind => {
                    prompt = IDLE_AFTER_BIND_CONTINUATION.to_string();
                }
                HostAdmissionStep::NudgeBeforeAnyGate => {
                    prompt = BEFORE_ANY_GATE_CONTINUATION.to_string();
                }
                HostAdmissionStep::NudgeUnprovenStop => {
                    prompt = UNPROVEN_STOP_CONTINUATION.to_string();
                }
                HostAdmissionStep::FreshContextReview => {
                    if let Err(error) = session.compact_conversation(Some(FRESH_REVIEW_FOCUS)).await
                    {
                        tracing::warn!("compact before the fresh reading skipped: {error}");
                    }
                    prompt = first_prompt(task);
                }
                HostAdmissionStep::RetryNonTerminal => {
                    let remembered = facts
                        .last_error
                        .clone()
                        .or_else(|| progress.last_error().map(str::to_owned));
                    if remembered
                        .as_deref()
                        .is_some_and(crate::unattended::result::is_provider_rejection)
                    {
                        bail!(
                            "{}",
                            remembered.unwrap_or_else(|| "provider rejected the request".into())
                        );
                    }
                    let only_generic =
                        |text: &str| crate::unattended::result::is_generic_stream_close(text);
                    let no_concrete = remembered.as_deref().is_none_or(only_generic)
                        && progress.last_error().is_none_or(only_generic);
                    // A joined worker with no concrete failure and no end event
                    // is its own outcome. Retrying it sixteen times only
                    // overwrites the record with the generic stream-closed text.
                    if join_failure.is_none() && no_concrete {
                        progress.mark_worker_exited_without_terminal_event();
                        bail!("A3S Code worker exited without a terminal event");
                    }
                    let detail = join_failure
                        .or(facts.last_error)
                        .unwrap_or_else(|| "stream closed without terminal event".to_string());
                    prompt = non_terminal_continuation(&detail);
                }
                HostAdmissionStep::NonTerminalExhausted => {
                    if let Some(failure) = join_failure.or_else(|| {
                        progress.last_error().and_then(|text| {
                            (!crate::unattended::result::is_generic_stream_close(text))
                                .then(|| text.to_owned())
                        })
                    }) {
                        bail!("{failure}");
                    }
                    progress.stream_closed_without_terminal_event = true;
                    bail!("A3S Code stream ended without a terminal event");
                }
                HostAdmissionStep::Finish => {
                    if !progress.terminal_event {
                        if let Some(failure) = join_failure {
                            bail!("join A3S Code stream: {failure}");
                        }
                        progress.stream_closed_without_terminal_event = true;
                        bail!("A3S Code stream ended without a terminal event");
                    }
                    if let Some(failure) = join_failure {
                        tracing::warn!("worker ended after terminal event: {failure}");
                    }
                    return Ok(None);
                }
            }
        }
    }

    /// One core stream, rendered to the client like a prompt turn and
    /// counted into `progress`. Cancel leaves exactly as `drain_turn` does.
    /// Past `remaining` the core run is cancelled, drained and the run fails
    /// on the execution deadline.
    #[allow(clippy::too_many_arguments)]
    async fn unattended_attempt(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        stream: a3s_code_core::AgentRunStream,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        progress: &mut RunProgress,
        remaining: std::time::Duration,
    ) -> anyhow::Result<Attempt> {
        let mut rx = stream.events;
        let join = stream.worker;
        let mut state = TurnState::default();
        let mut facts = AttemptFacts::default();
        let session_key = session_id.0.to_string();
        let deadline = tokio::time::sleep(remaining);
        tokio::pin!(deadline);
        let mut expired = false;
        loop {
            let notified = self.turn_cancel.notified();
            if self.turn_was_cancelled(&session_key, generation).await {
                let response = self
                    .leave_cancelled_turn(
                        session_id,
                        session,
                        &mut state,
                        &mut rx,
                        join,
                        turn_permit,
                    )
                    .await;
                return Ok(Attempt::Cancelled(response));
            }
            tokio::select! {
                biased;
                _ = notified => continue,
                _ = &mut deadline, if !expired => {
                    expired = true;
                    tracing::warn!("run deadline reached mid-attempt; cancelling the core run");
                    session.cancel().await;
                }
                event = rx.recv() => match event {
                    Some(event) => {
                        note_event(progress, &mut facts, &event);
                        self.handle_event(session_id, session, &mut state, event).await;
                    }
                    None => break,
                },
            }
        }
        progress.phase = ExecutionPhase::Joining;
        let join_failure = join.await.err().map(worker_join_failure);
        if let Some(failure) = &join_failure {
            progress.remember_error(failure);
            facts.failed = true;
        }
        if let Some(outcome) = state.end_outcome.take() {
            self.emit_turn_outcome(session_id, outcome).await;
        }
        if expired {
            bail!("A3S Code execution deadline exceeded");
        }
        Ok(Attempt::Ended {
            turn_permit,
            facts,
            join_failure,
        })
    }
}
