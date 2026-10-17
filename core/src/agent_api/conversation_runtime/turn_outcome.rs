use super::*;

pub(super) async fn settle_owned_capability_run(
    pinned: &OwnedPinnedFact,
    outcome: std::result::Result<AgentResult, anyhow::Error>,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let cancelled_before_cleanup = pinned
        .parts
        .surface
        .as_ref()
        .is_some_and(|surface| surface.cancel.is_cancelled());
    // The last tool round's turn is a child of this run. Close it first so
    // the run's cleanup is not waiting on a scope this slot still holds.
    if let Some(surface) = &pinned.parts.surface {
        if let Some(turn) = surface.agent.fact_turn.lock().await.take() {
            if let Err(error) = turn.close().await {
                tracing::warn!(error = %error, "fact turn close failed before run close");
            }
        }
    }
    let closed = pinned.guard.close().await;
    if let Err(error) = pinned
        .original
        .record_scope_close(run_admission::OriginalScopeKind::Fact, &closed)
    {
        return Err(anyhow::anyhow!("{error}; original result: {outcome:?}"));
    }
    apply_capability_close(&pinned.parts, outcome, cancelled_before_cleanup, closed).await
}

pub(super) async fn apply_capability_close(
    parts: &FactSession,
    outcome: std::result::Result<AgentResult, anyhow::Error>,
    cancelled_before_cleanup: bool,
    closed: std::result::Result<
        crate::capability::ScopeCloseReport,
        crate::capability::CapabilityRuntimeError,
    >,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let cleanup_succeeded = closed.is_ok();
    let outcome = match (outcome, closed) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Err(close)) => Err(anyhow::anyhow!(
            "{error:#}; capability Run cleanup also failed: {close}"
        )),
    };
    if let Some(surface) = parts.surface.as_ref().filter(|_| cleanup_succeeded) {
        crate::fact_control::replay_unsettled_mutations(surface);
        let mut ledger = surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let observation_cancel = tokio_util::sync::CancellationToken::new();
        let incomplete = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::harness_loop::absorb_open_workspace_children(
                &mut ledger,
                parts.workspace.as_path(),
                &observation_cancel,
            ),
        )
        .await
        .unwrap_or(true);
        if incomplete {
            ledger.set_observation_incomplete(true);
        }
        *surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ledger;
        // Local child observation cannot clear durable execution debt. Once
        // children are settled, re-project their exact committed terminals.
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    match outcome {
        Ok(_)
            if !cancelled_before_cleanup
                && parts
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.cancel.is_cancelled()) =>
        {
            Err(anyhow::anyhow!(
                "Agent Run interrupted during capability cleanup"
            ))
        }
        Ok(result) => Ok(result),
        Err(error) => Err(error),
    }
}

pub(super) fn strip_attachment_annotation(messages: &mut [Message]) {
    let Some(message) = messages.first_mut() else {
        return;
    };
    if message.role != "user" {
        return;
    }
    let text = message.text();
    let Some(visible) = text.split("\n[attachment ").next() else {
        return;
    };
    if visible != text {
        *message = Message::user(visible);
    }
}

pub(super) fn tool_results_since_last_user_message(log: &[a3s_effect::Fact]) -> usize {
    let start = log
        .iter()
        .rposition(|fact| fact.kind == "user.message")
        .unwrap_or(0);
    log[start..]
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .count()
}

/// The effect machine mints `budget.denied` when the tool-round budget
/// denies the next call. Exact fact-kind lookup on this turn's slice (since
/// the last `user.message`) — never a text-shape match on the answer.
pub(super) fn budget_denied_this_turn(
    parts: &FactSession,
) -> Option<crate::harness_loop::ForcedStop> {
    let thread = crate::fact_control::thread_for_session(&parts.session_id);
    let facts = crate::fact_control::read_workspace_facts(&parts.workspace, &thread).ok()?;
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "user.message")
        .unwrap_or(0);
    facts[start..]
        .iter()
        .any(|fact| fact.kind == "budget.denied")
        .then_some(crate::harness_loop::ForcedStop::ToolBudget)
}

pub(super) fn captured_forced_stop(parts: &FactSession) -> Option<crate::harness_loop::ForcedStop> {
    parts
        .surface
        .as_ref()
        .and_then(|surface| {
            surface
                .forced_stop
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        })
        .or_else(|| budget_denied_this_turn(parts))
}

pub(super) fn combine_retention_failure(
    outcome: std::result::Result<AgentResult, anyhow::Error>,
    stage: &str,
    error: impl std::fmt::Display,
) -> std::result::Result<AgentResult, anyhow::Error> {
    match outcome {
        Ok(_) => Err(anyhow::anyhow!("{stage} failed: {error}")),
        Err(primary) => Err(anyhow::anyhow!("{primary:#}; {stage} also failed: {error}")),
    }
}

pub(super) fn gate_result(
    parts: &FactSession,
    result: AgentResult,
    goal_attestation: Option<&[crate::verification::VerificationReport]>,
) -> std::result::Result<AgentResult, anyhow::Error> {
    match &parts.surface {
        Some(surface) => gate_surface_result(surface, result, goal_attestation),
        None => Ok(result),
    }
}

/// Close a settled run through the completion gate of its surface.
pub(crate) fn gate_surface_result(
    surface: &crate::fact_control::SessionSurface,
    mut result: AgentResult,
    goal_attestation: Option<&[crate::verification::VerificationReport]>,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(fresh) = goal_attestation {
        if !ledger.observes_current_workspace(&surface.agent.tool_context_handle().workspace) {
            anyhow::bail!("goal.evidence_stale: workspace effects changed after judgment");
        }
        reports.extend_from_slice(fresh);
    } else {
        surface
            .agent
            .merge_completion_attestor(&ledger, &mut reports);
    }
    if let Ok(mut lock) = surface.reports.lock() {
        *lock = reports.clone();
    }
    result.verification_reports = reports.clone();
    match surface.agent.decide_fact_completion_gate(&ledger, &reports) {
        crate::harness_loop::CompletionGate::Allow(terminal) => {
            // The run closed accountable: mark the log position so the next
            // send's ledger replay starts here instead of inheriting this
            // run's already-verified mutations.
            crate::fact_control::append_turn_complete_fact(surface, &terminal)?;
            result.completion = terminal;
            Ok(result)
        }
        crate::harness_loop::CompletionGate::Incomplete { message }
        | crate::harness_loop::CompletionGate::Continue { message } => {
            Err(anyhow::anyhow!(message))
        }
    }
}

pub(super) fn agent_result(
    settled: &a3s_effect::Settlement<a3s_effect::CodingView>,
    messages: Vec<Message>,
    usage: crate::llm::TokenUsage,
) -> AgentResult {
    AgentResult {
        text: settled.view.assistant.clone().unwrap_or_default(),
        messages,
        usage,
        tool_calls_count: tool_results_since_last_user_message(&settled.log),
        verification_reports: Vec::new(),
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        run_admission: "ordinary".into(),
        forced_stop: None,
    }
}

#[cfg(test)]
pub(super) async fn prepare_pinned(
    session: &AgentSession,
    prompt: &str,
    owned_run: bool,
) -> Result<PinnedFact> {
    // Same capture point as `OwnedConversationCapture`: host reports recorded
    // before this turn must already be on the surface the gate reads.
    let recorded_reports = session.verification_reports();
    let cancellation = session.session_cancel.child_token();
    let (mut agent_loop, capability_run) =
        build_pinned_agent_loop(session, cancellation.clone()).await?;
    let mut committer = None;
    let mut checkpoint = None;
    let mut active_run_id = None;
    let mut run_control = None;
    if owned_run {
        let Some(binding) = agent_loop.checkpoint_capability_binding().cloned() else {
            let _ = capability_run.close().await;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        };
        let cognitive = cognitive_binding_for_projection(session, capability_run.projection());
        let run = match RunControlState::from_session(session)
            .start_run_with_bindings(prompt, cognitive.clone(), binding.clone())
            .await
        {
            Ok(run) => {
                active_run_id = Some(run.id().to_string());
                run
            }
            Err(error) => {
                let _ = capability_run.close().await;
                return Err(error);
            }
        };
        if let Some(binding) = cognitive {
            session
                .run_store
                .record_event(
                    run.id(),
                    crate::agent::AgentEvent::CognitiveContextBound { binding },
                )
                .await;
        }
        let started = AgentEvent::Start {
            prompt: prompt.to_string(),
        };
        session
            .run_store
            .record_event(run.id(), started.clone())
            .await;
        if let Some(hook) = &session.hook_executor {
            hook.record_agent_event(&started, run.id(), &session.session_id)
                .await;
        }
        let (checkpoint_sink, mut checkpoints) = runtime_checkpoint_channel(session);
        if let Some(sink) = checkpoint_sink {
            checkpoint = Some(crate::fact_control::CheckpointEmit {
                sink,
                run_id: run.id().to_string(),
                session_id: session.session_id.clone(),
                capability_binding: Some(binding),
            });
            committer = Some(tokio::spawn(async move {
                while let Some(boundary) = checkpoints.recv().await {
                    checkpoints.commit(boundary).await;
                }
            }));
        } else {
            drop(checkpoints);
        }
        let prepared =
            ExecutionCoordinator::prepare(session, run.id(), &mut agent_loop, cancellation.clone())
                .await;
        prepared.run_control().update_turn(1).await;
        run_control = Some(prepared.run_control());
    }
    let streamed_text = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let forced_stop = std::sync::Arc::new(std::sync::Mutex::new(None));
    let _ = &forced_stop;
    let parts = FactSession::from(session)
        .with_executor(agent_loop.tool_executor_handle())
        .with_surface(crate::fact_control::SessionSurface {
            agent: agent_loop,
            session_id: session.session_id.clone(),
            checkpoint,
            events: None,
            pending_memory_telemetry: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            fact_log: session.run_admission.fact_log_slot(),
            streamed_text: streamed_text.clone(),
            forced_stop: forced_stop.clone(),
            cancel: cancellation.clone(),
            transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
            usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
            confirmation: session.config.confirmation_manager.clone(),
            run_store: active_run_id
                .as_ref()
                .map(|_| Arc::clone(&session.run_store)),
            run_id: active_run_id.clone(),
            goal_tick: None,
            ledger: Arc::new(std::sync::Mutex::new(
                crate::harness_loop::MutationLedger::default(),
            )),
            reports: Arc::new(std::sync::Mutex::new(recorded_reports)),
            run_control,
            targeted_paths: Arc::new(std::sync::Mutex::new(Vec::new())),
            harness: session.config.harness.clone(),
            host_harness_registry: session.config.host_harness_registry.clone(),
            host_harness_assembler: session.config.host_harness_assembler.clone(),
        });
    // Inherit the mutations of runs that never settled accountable (a
    // graceful interrupt or a gate refusal): the fresh surface must not
    // open with a blank ledger over a changed workspace. A checkpoint
    // resume conservatively merges its cached projection into this seed next.
    if let Some(surface) = parts.surface.as_ref() {
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    if owned_run {
        if let Some(cancel) = parts.surface.as_ref().map(|surface| surface.cancel.clone()) {
            *session.cancel_token.lock().await = Some(cancel);
        }
    }
    Ok(PinnedFact {
        parts,
        capability_run,
        committer,
    })
}
