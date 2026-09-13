use super::completion_runtime::CompletionFlow;
use super::execution_state::ExecutionLoopState;
use super::llm_turn::LlmTurnRequest;
use super::queue_forwarder::QueueEventForwarder;
use super::{AgentEvent, AgentLoop, AgentResult};
use crate::llm::{ContentBlock, Message};
use crate::prompts::AgentStyle;
use anyhow::Result;
use tokio::sync::mpsc;

const TOOL_BUDGET_FINALIZATION: &str = "Tool-use budget reached. Stop gathering evidence and return the best complete final answer now using only the tool results already present. Do not call any tool.";

impl AgentLoop {
    /// Core execution loop (without planning routing).
    ///
    /// This is the inner loop that runs LLM calls and tool executions.
    /// Called directly by `execute_with_session` (after planning check)
    /// and by `execute_plan` (for individual steps, bypassing planning).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_loop(
        &self,
        history: &[Message],
        prompt: &str,
        effective_style: AgentStyle,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
        emit_end: bool,
    ) -> Result<AgentResult> {
        // When called via execute_loop, the prompt is used for both
        // message-adding and hook/memory/event purposes.
        self.execute_loop_inner(
            history,
            prompt,
            prompt,
            Some(effective_style),
            session_id,
            event_tx,
            cancel_token,
            emit_end,
            None,
        )
        .await
    }

    /// Inner execution loop.
    ///
    /// `msg_prompt` controls whether a user message is appended (empty = skip).
    /// `effective_prompt` is used for hooks, memory recall, taint tracking, and events.
    /// `effective_style` pre-computed style to skip redundant LLM-based intent detection.
    /// `emit_end` controls whether to send `AgentEvent::End` when the loop completes
    /// (should be false when called from `execute_plan` to avoid duplicate End events).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_loop_inner(
        &self,
        history: &[Message],
        msg_prompt: &str,
        effective_prompt: &str,
        effective_style: Option<AgentStyle>,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
        emit_end: bool,
        seed: Option<super::execution_state::ExecutionSeed>,
    ) -> Result<AgentResult> {
        let mut state = ExecutionLoopState::new_seeded(history, seed);
        state
            .bind_run_watch(self.tool_context.workspace.as_path())
            .await;
        if state.open_observations.is_empty() {
            state.open_observations = self.config.external_observations.clone();
        }

        let style_prompt = if effective_prompt.is_empty() {
            msg_prompt
        } else {
            effective_prompt
        };
        let prompt_mode = self
            .resolve_prompt_mode(effective_style, style_prompt, &event_tx)
            .await;
        let effective_system_prompt = prompt_mode.system_prompt;

        // Send start event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::Start {
                prompt: effective_prompt.to_string(),
            })
            .await
            .ok();
        }

        let _queue_forwarder = QueueEventForwarder::start(
            self.command_queue.as_ref(),
            event_tx.as_ref(),
            cancel_token,
        );

        let prompt_before_hooks = effective_prompt;
        // Lock product-visible text from the entry prompt BEFORE PrePrompt can
        // append model-only context. Wire mutations after this point must not
        // redefine the user bubble.
        let entry_product_text = if msg_prompt.is_empty() {
            None
        } else {
            let display = crate::transcript::product_user_text(msg_prompt);
            (!display.is_empty()).then_some(display)
        };
        let turn_context = match self
            .prepare_turn_context(
                &effective_system_prompt,
                effective_prompt,
                state.messages.len(),
                session_id,
                &event_tx,
            )
            .await
        {
            Ok(context) => context,
            Err(error) => return Err(state.finish_failed(error)),
        };
        let effective_prompt = turn_context.effective_prompt.as_str();
        let augmented_system = turn_context.augmented_system;

        self.config.rl_trajectory_recorder.record_execution_start(
            crate::rl_trajectory::ExecutionStartRecord {
                session_id: session_id.unwrap_or(""),
                workspace: &self.tool_context.workspace,
                prompt: effective_prompt,
                history,
                system_prompt: augmented_system.as_deref(),
                max_tool_rounds: self.config.max_tool_rounds,
                planning_mode: &format!("{:?}", self.config.planning_mode),
            },
        );

        // Put the hook-effective prompt on the wire. Previously the rewritten
        // value was used only for context lookup and telemetry while the LLM
        // still received the original user message, making prompt rewrites and
        // additionalContext observational instead of authoritative.
        if !msg_prompt.is_empty() {
            state.messages.push(match entry_product_text.as_deref() {
                Some(display) => Message::user_for_model_with_transcript(effective_prompt, display),
                None => product_or_wire_user_message(effective_prompt),
            });
        } else if effective_prompt != prompt_before_hooks {
            rewrite_latest_user_prompt(&mut state.messages, prompt_before_hooks, effective_prompt);
        }

        // The invocation owns the control inbox for this run.  Keeping the
        // handle local makes every safe-point operation cheap and ensures a
        // standalone AgentLoop (which has no invocation binding) behaves
        // exactly as before.
        let run_control = self
            .bound_invocation
            .as_ref()
            .and_then(|invocation| invocation.run_control());

        loop {
            // `max_tool_rounds` bounds evidence gathering, not the agent's
            // ability to return the evidence it already collected. Reserve one
            // provider turn with an empty tool set so bounded child runs
            // converge instead of discarding all work at the limit.
            let force_finalization = state.current_turn() >= self.config.max_tool_rounds;
            if force_finalization {
                state
                    .messages
                    .push(Message::user_wire(TOOL_BUDGET_FINALIZATION));
            }
            let turn = state.next_turn();
            if let Some(control) = &run_control {
                let snapshot = control.update_turn(turn).await;
                Self::apply_pending_run_controls(
                    control,
                    &mut state,
                    &event_tx,
                    &snapshot,
                    self.config.host_env.now_ms(),
                )
                .await;
                // An interrupt is cooperative but should prevent opening a
                // new provider/capability turn once it has been observed.
                if cancel_token.is_cancelled() {
                    return self.finish_cancel(state, cancel_token).await;
                }
            }
            let capability_turn = match self
                .capability_runtime
                .as_ref()
                .map(|runtime| runtime.begin_turn(turn))
                .transpose()
            {
                Ok(turn) => turn,
                Err(error) => return Err(state.finish_failed(error.into())),
            };
            let scoped_cancellation = capability_turn
                .as_ref()
                .map(crate::capability::AgentCapabilityTurn::cancellation)
                .unwrap_or_else(|| cancel_token.clone());
            let scoped_tool_context = capability_turn.as_ref().map_or_else(
                || self.tool_context.clone(),
                |scope| {
                    self.tool_context
                        .clone()
                        .with_capability_context(scope.tool_context())
                },
            );
            self.inject_turn_scoped_evidence(augmented_system.as_deref().unwrap_or(""), &mut state);
            let verifier_role = state.next_turn_is_verifier;
            let llm_turn = match self
                .execute_llm_turn(
                    &mut state,
                    LlmTurnRequest {
                        turn,
                        augmented_system: &augmented_system,
                        effective_prompt,
                        session_id,
                        event_tx: &event_tx,
                        cancel_token: &scoped_cancellation,
                        force_no_tools: force_finalization,
                        verifier_role,
                    },
                )
                .await
            {
                Ok(turn) => turn,
                // Interrupted mid-generation (Esc / cancel): keep the conversation
                // accumulated so far — above all the user's message — and return it
                // as the result so it is committed to history. Without this the
                // whole turn is dropped and the agent "forgets" what was just asked
                // when the user continues.
                Err(_) if scoped_cancellation.is_cancelled() => {
                    if let Some(control) = &run_control {
                        let snapshot = control.snapshot().await;
                        Self::apply_pending_run_controls(
                            control,
                            &mut state,
                            &event_tx,
                            &snapshot,
                            self.config.host_env.now_ms(),
                        )
                        .await;
                    }
                    if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                        return Err(state.finish_failed(error));
                    }
                    release_session_shell(session_id);
                    return self.finish_cancel(state, &scoped_cancellation).await;
                }
                Err(error) => {
                    if let Err(close_error) = close_capability_turn(capability_turn.as_ref()).await
                    {
                        tracing::warn!(
                            error = %close_error,
                            "Capability Turn close also failed after provider failure"
                        );
                    }
                    return Err(state.finish_failed(error));
                }
            };
            debug_assert_eq!(llm_turn.turn, turn);
            let response = llm_turn.response;
            let tool_calls = llm_turn.tool_calls;

            if force_finalization && !tool_calls.is_empty() {
                let error = format!(
                    "Max tool rounds ({}) exceeded; the reserved finalization turn attempted another tool call",
                    self.config.max_tool_rounds
                );
                self.emit_error(&event_tx, error.clone()).await;
                if let Err(close_error) = close_capability_turn(capability_turn.as_ref()).await {
                    tracing::warn!(
                        error = %close_error,
                        "Capability Turn close also failed after finalization violation"
                    );
                }
                return Err(state.finish_failed(anyhow::anyhow!(error)));
            }

            if tool_calls.is_empty() {
                match self
                    .complete_no_tool_response(
                        &mut state,
                        turn,
                        &response,
                        effective_prompt,
                        session_id,
                        &event_tx,
                        emit_end,
                        &scoped_cancellation,
                        &scoped_tool_context,
                        force_finalization,
                    )
                    .await
                {
                    CompletionFlow::Continue => {
                        if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                            return Err(state.finish_failed(error));
                        }
                        continue;
                    }
                    CompletionFlow::Finished {
                        text,
                        completion,
                        run_admission,
                    } => {
                        if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                            return Err(state.finish_failed(error));
                        }
                        return Ok(state.finish_with(text, completion, run_admission));
                    }
                    CompletionFlow::Blocked(message) => {
                        if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                            return Err(state.finish_failed(error));
                        }
                        return Err(state.finish_failed(anyhow::anyhow!(message)));
                    }
                }
            }

            let tool_dispatch_context = if verifier_role {
                scoped_tool_context.clone().with_verifier_read_only(true)
            } else {
                scoped_tool_context.clone()
            };
            if let Err(e) = self
                .execute_tool_turn(
                    tool_calls,
                    &mut state,
                    &event_tx,
                    session_id,
                    &scoped_cancellation,
                    &tool_dispatch_context,
                )
                .await
            {
                // Same as above: a cancelled tool round commits its partial
                // history rather than being dropped.
                if scoped_cancellation.is_cancelled() {
                    if let Some(control) = &run_control {
                        let snapshot = control.snapshot().await;
                        Self::apply_pending_run_controls(
                            control,
                            &mut state,
                            &event_tx,
                            &snapshot,
                            self.config.host_env.now_ms(),
                        )
                        .await;
                    }
                    if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                        return Err(state.finish_failed(error));
                    }
                    release_session_shell(session_id);
                    return self.finish_cancel(state, &scoped_cancellation).await;
                }
                if let Err(close_error) = close_capability_turn(capability_turn.as_ref()).await {
                    tracing::warn!(
                        error = %close_error,
                        "Capability Turn close also failed after Tool failure"
                    );
                }
                return Err(state.finish_failed(e));
            }

            if let Err(error) = close_capability_turn(capability_turn.as_ref()).await {
                return Err(state.finish_failed(error));
            }
            state.next_turn_is_verifier = false;
            // Quiescent boundary: all tools and capability-owned effects have
            // settled, and `state.messages` is consistent. The runtime sink
            // drains every preceding event before acknowledging persistence.
            self.persist_loop_checkpoint(turn, &state, session_id).await;
        }
    }

    /// Consume controls only at loop safe points. Steering becomes a normal
    /// user message in the run-owned transcript; interrupt requests have
    /// already fired the run cancellation token when accepted.
    async fn apply_pending_run_controls(
        control: &crate::run_control::RunControlInbox,
        state: &mut ExecutionLoopState,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        snapshot: &crate::run_control::RunControlSnapshot,
        now_ms: u64,
    ) {
        let pending = control.drain().await;
        for pending in pending {
            let (input, reason) = match &pending.request.command {
                crate::run_control::RunControlCommand::Steer { input } => {
                    (Some(input.clone()), None)
                }
                crate::run_control::RunControlCommand::Interrupt { reason, .. } => {
                    (None, reason.clone())
                }
            };
            let receipt = control
                .mark_applied(
                    &pending,
                    snapshot.turn_id.clone(),
                    snapshot.turn_revision,
                    now_ms,
                )
                .await;
            // A session close can settle a drained request before this safe
            // point is acknowledged. Only mutate the loop transcript after
            // the inbox confirms that this invocation won the race.
            if receipt.state != crate::run_control::RunControlReceiptState::Applied {
                continue;
            }
            if let Some(input) = &input {
                state.messages.push(Message::user(input));
            }
            if let Some(tx) = event_tx {
                tx.send(AgentEvent::RunControlApplied {
                    request_id: receipt.request_id,
                    operation: receipt.operation,
                    turn_id: receipt.turn_id,
                    turn_revision: receipt.turn_revision,
                    input,
                    reason,
                })
                .await
                .ok();
            }
        }
    }

    /// Persist a `LoopCheckpoint` if both a sink and a bound run id are
    /// configured. Failures are swallowed (the sink already logs them)
    /// so an unavailable store cannot halt a live run.
    async fn persist_loop_checkpoint(
        &self,
        turn: usize,
        state: &super::execution_state::ExecutionLoopState,
        session_id: Option<&str>,
    ) {
        let Some(sink) = self.checkpoint_sink.as_ref() else {
            return;
        };
        let Some(run_id) = self.checkpoint_run_id.as_ref() else {
            return;
        };
        let checkpoint = crate::loop_checkpoint::LoopCheckpoint {
            schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
            run_id: run_id.clone(),
            session_id: session_id.unwrap_or("").to_string(),
            capability_binding: self.checkpoint_capability_binding.clone(),
            turn,
            messages: state.messages.clone(),
            total_usage: state.total_usage.clone(),
            tool_calls_count: state.tool_calls_count,
            verification_reports: state.verification_reports.clone(),
            convergence: state.convergence_checkpoint(),
            checkpoint_ms: self.config.host_env.now_ms(),
        };
        sink.save_checkpoint(&checkpoint).await;
    }

    fn inject_turn_scoped_evidence(&self, system_prompt: &str, state: &mut ExecutionLoopState) {
        let injection = crate::path_instructions::inject(
            system_prompt,
            &self.config.path_rules,
            &state.targeted_paths,
        );
        if let Some(fragment) = injection.model_input_fragment() {
            if !state
                .messages
                .iter()
                .any(|message| message.text().contains(&injection.injected_digest))
            {
                state.messages.push(Message::user_wire(&fragment));
            }
        }
        for observation in &state.open_observations {
            let fragment = observation.model_input_fragment();
            if !state
                .messages
                .iter()
                .any(|message| message.text().contains(&observation.digest))
            {
                state.messages.push(Message::user_wire(&fragment));
            }
        }
    }

    /// Cancellation of a read-only run may keep history. A mutated workspace
    /// is still incomplete and must not return `AgentResult` success.
    pub(super) async fn finish_cancel(
        &self,
        mut state: ExecutionLoopState,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<AgentResult> {
        let workspace = self.tool_context.workspace.as_path();
        let child_unobserved = crate::harness_loop::absorb_open_workspace_children(
            &mut state.mutations,
            workspace,
            cancel,
        )
        .await;
        let unseen = state.unseen_workspace_paths(workspace).await;
        state
            .mutations
            .set_observation_incomplete(child_unobserved || unseen.incomplete);
        state.mutations.observe_unseen_paths(&unseen.paths);
        if state.mutations.observation_incomplete() {
            return Err(state.finish_failed(anyhow::anyhow!(
                "completion gate: workspace observation is incomplete, so this digest is not the effect. Cancellation does not make that a success."
            )));
        }
        if state.mutations.is_empty() && !state.mutations.has_open_children() {
            return Ok(state.finish_interrupted());
        }
        let message = if state.mutations.has_open_children() {
            "completion gate: background workspace work is still unobserved. Cancellation does not make that a success.".to_string()
        } else {
            format!(
                "completion gate: workspace mutation {} has no bound Passed verification and no host waiver. Cancellation does not make that a success.",
                state.mutations.digest()
            )
        };
        Err(state.finish_failed(anyhow::anyhow!(message)))
    }
}

fn release_session_shell(session_id: Option<&str>) {
    if let Some(session_id) = session_id {
        crate::shell_session::kill_session(session_id);
    }
}

async fn close_capability_turn(
    turn: Option<&crate::capability::AgentCapabilityTurn>,
) -> anyhow::Result<()> {
    let Some(turn) = turn else {
        return Ok(());
    };
    let report = turn.close().await?;
    if !report.is_clean() {
        anyhow::bail!(
            "Capability Turn close was incomplete (tasks failed: {}, tasks timed out: {}, child scopes failed: {}, child scopes timed out: {}, effects failed: {}, effects timed out: {})",
            report.tasks_failed,
            report.tasks_timed_out,
            report.child_scopes_failed,
            report.child_scopes_timed_out,
            report.effects_failed,
            report.effects_timed_out,
        );
    }
    Ok(())
}

fn rewrite_latest_user_prompt(messages: &mut [Message], original: &str, replacement: &str) {
    let candidate = messages
        .iter()
        .rposition(|message| {
            message.role == "user" && message.is_product_transcript() && message.text() == original
        })
        .or_else(|| {
            messages.iter().rposition(|message| {
                message.role == "user"
                    && message.is_product_transcript()
                    && !message.text().is_empty()
            })
        });
    let Some(index) = candidate else {
        tracing::warn!("PrePrompt modified input but no user message could be rewritten");
        return;
    };
    let message = &mut messages[index];
    // Freeze product-visible text from the pre-rewrite body only. Never copy
    // raw wire text when product_user_text cannot recover a human sentence —
    // that path used to permanently publish hook/planner appendices.
    if message.transcript_text.is_none() {
        let display = crate::transcript::product_user_text(&message.text());
        if !display.is_empty() {
            message.transcript_text = Some(display);
        }
    }

    let mut wrote_text = false;
    message.content.retain_mut(|block| match block {
        ContentBlock::Text { text } if !wrote_text => {
            *text = replacement.to_string();
            wrote_text = true;
            true
        }
        ContentBlock::Text { .. } => false,
        _ => true,
    });
    if !wrote_text {
        message.content.push(ContentBlock::Text {
            text: replacement.to_string(),
        });
    }
}

fn product_or_wire_user_message(text: &str) -> Message {
    let display = crate::transcript::product_user_text(text);
    if display.is_empty() {
        Message::user_wire(text)
    } else {
        Message::user_for_model_with_transcript(text, &display)
    }
}

#[cfg(test)]
mod tests {
    use super::super::execution_state::ExecutionLoopState;
    use super::super::tests::MockLlmClient;
    use super::super::{AgentConfig, AgentLoop};
    use crate::path_instructions::PathRule;
    use crate::tools::{ToolContext, ToolExecutor};
    use std::sync::Arc;

    #[test]
    fn dotted_tool_path_injects_the_matching_rule_before_the_next_model_call() {
        let hermetic_root = crate::test_support::hermetic_workspace();
        let config = AgentConfig {
            path_rules: vec![
                PathRule {
                    glob: "docs/**".into(),
                    text: "Docs tone.".into(),
                },
                PathRule {
                    glob: "crates/code/**".into(),
                    text: "Prefer boring harness code.".into(),
                },
            ],
            ..Default::default()
        };
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new(hermetic_root.display().to_string())),
            ToolContext::new(hermetic_root.clone()),
            config,
        );
        let mut state = ExecutionLoopState::new_seeded(&[], None);
        state
            .targeted_paths
            .push("docs/../crates/code/core/src/lib.rs".into());
        let system = "# Instructions\nproject AGENTS.md";
        agent.inject_turn_scoped_evidence(system, &mut state);
        agent.inject_turn_scoped_evidence(system, &mut state);
        let text = state
            .messages
            .iter()
            .map(|message| message.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Prefer boring harness code."));
        assert!(!text.contains("Docs tone."));
        assert!(!text.contains("project AGENTS.md"));
        assert_eq!(state.messages.len(), 1);
    }
}
