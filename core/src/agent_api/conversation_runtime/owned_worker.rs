use super::*;

pub(super) async fn owned_conversation_worker(
    mut captured: OwnedConversationCapture,
    request: OwnedConversationRequest,
    mode: run_admission::ExecutionMode,
    original: Arc<run_admission::OriginalExecutionContext>,
    ready: tokio::sync::oneshot::Sender<Result<OwnedReady>>,
    result: tokio::sync::oneshot::Sender<Result<AgentResult>>,
    events: mpsc::Sender<AgentEvent>,
) {
    use run_admission::OriginalScopeKind as Scope;
    let mut ready = Some(ready);
    let bind_source_generation =
        matches!(&request, OwnedConversationRequest::PreparedRecovery { .. });
    let mut request = request;
    let goal_tick = match &mut request {
        OwnedConversationRequest::Prompt { goal_tick, .. } => goal_tick.take(),
        _ => None,
    };
    let resolved: Result<_> = async {
        let request_inputs: Result<_> = async {
            let resolved = match request {
                OwnedConversationRequest::Prompt {
                    prompt,
                    attachments,
                    history,
                    command_capable,
                    exact_run_id,
                    ..
                } => (
                    prompt,
                    attachments,
                    history,
                    command_capable,
                    exact_run_id,
                    None,
                    false,
                ),
                OwnedConversationRequest::ExactRecovery {
                    source,
                    target,
                    prompt,
                } => {
                    let checkpoint = owned_load_checkpoint(
                        captured.preparation.checkpoint.store.as_ref(),
                        &captured.preparation.session_id,
                        &source,
                    )
                    .await?
                    .ok_or_else(|| {
                        CodeError::Session(format!("no loop checkpoint found for run '{source}'"))
                    })?;
                    (
                        prompt,
                        Vec::new(),
                        None,
                        false,
                        Some(target),
                        Some(checkpoint),
                        false,
                    )
                }
                OwnedConversationRequest::PreparedRecovery {
                    checkpoint,
                    target,
                    prompt,
                } => (
                    prompt,
                    Vec::new(),
                    None,
                    false,
                    Some(target),
                    Some(*checkpoint),
                    false,
                ),
                OwnedConversationRequest::OrdinaryResume { source } => {
                    let checkpoint = owned_load_checkpoint(
                        captured.preparation.checkpoint.store.as_ref(),
                        &captured.preparation.session_id,
                        &source,
                    )
                    .await?;
                    if checkpoint.is_none() {
                        let thread = crate::fact_control::thread_for_session(
                            &captured.preparation.session_id,
                        );
                        let empty = crate::fact_control::read_workspace_facts(
                            &captured.preparation.workspace,
                            &thread,
                        )
                        .map(|facts| facts.is_empty())
                        .unwrap_or(true);
                        if empty {
                            return Err(CodeError::Session(
                                if captured.preparation.checkpoint.store.is_none() {
                                    "resume_run requires a session_store on this session".to_owned()
                                } else {
                                    format!("no loop checkpoint found for run '{source}'")
                                },
                            ));
                        }
                    }
                    let prompt = checkpoint
                        .as_ref()
                        .and_then(|checkpoint| {
                            checkpoint
                                .messages
                                .iter()
                                .find(|message| message.role == "user")
                        })
                        .map(Message::text)
                        .unwrap_or_default();
                    (prompt, Vec::new(), None, false, None, checkpoint, true)
                }
            };
            if let Some(checkpoint) = &resolved.5 {
                captured
                    .preparation
                    .checkpoint
                    .persistence
                    .validate_checkpoint_source(checkpoint)?;
            }
            Ok(resolved)
        }
        .await;
        let (mut prompt, attachments, history, command_capable, exact, checkpoint, ordinary) =
            match request_inputs {
                Ok(actual) => actual,
                Err(error) => {
                    // Only read-only request resolution occurred; neither scope,
                    // Run, inbox nor committer was constructed on this branch.
                    original.record_scope_not_attempted(Scope::Command)?;
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    return Err(error);
                }
            };
        if command_capable {
            let is_command = crate::commands::CommandRegistry::is_command(&prompt);
            if is_command {
                let command = captured.preparation.command.take().ok_or_else(|| {
                    CodeError::Session("original command projection missing".into())
                })?;
                original.mark_scope_attempted(Scope::Command)?;
                let (runtime, guard, _) =
                    match super::super::agent_loop_runtime::admit_captured_runtime_projection(
                        command,
                        &captured.preparation.boundary,
                        captured.preparation.cancellation.clone(),
                        &captured.preparation.workspace,
                    )
                    .await
                    {
                        Ok(actual) => actual,
                        Err(failure) => {
                            let (error, actual) = failure.into_original_observation();
                            if let Some((guard, close)) = actual {
                                original.record_scope_returned(Scope::Command, &guard)?;
                                original.record_scope_close(Scope::Command, &close)?;
                            }
                            original.record_scope_not_attempted(Scope::Fact)?;
                            let cleanup = owned_no_run_finish(&captured, &original).await;
                            return Err(combine_original_failure(error, cleanup));
                        }
                    };
                original.record_scope_returned(Scope::Command, &guard)?;
                let prepared =
                    command_runtime::prepare_owned(&captured.commands, &prompt, &runtime, guard)
                        .await;
                let (guard, closed, text) = prepared.into_original_observation();
                original.record_scope_close(Scope::Command, &closed)?;
                drop(guard);
                if let Err(error) = closed {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    let cleanup = owned_no_run_finish(&captured, &original).await;
                    return Err(combine_original_failure(error.into(), cleanup));
                }
                if let Some(text) = text {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(OwnedReady::Command));
                    }
                    let answer = captured
                        .commands
                        .blocking_result(text.clone(), history.as_deref());
                    if mode != run_admission::ExecutionMode::Blocking {
                        command_runtime::send_text_output(&events, text).await;
                    }
                    return Ok(Some(answer));
                }
            } else {
                original.record_scope_not_attempted(Scope::Command)?;
            }
        } else {
            original.record_scope_not_attempted(Scope::Command)?;
        }
        if mode == run_admission::ExecutionMode::Blocking && command_capable {
            if let Some(warning) = &captured.init_warning {
                tracing::warn!(
                    session_id = %captured.preparation.session_id,
                    "Session init warning: {}", warning
                );
            }
        }
        // Debt is read from the original workspace inside the owned worker.
        if mode == run_admission::ExecutionMode::Streaming || ordinary {
            let thread = crate::fact_control::thread_for_session(&captured.preparation.session_id);
            let owed =
                crate::fact_control::read_workspace_facts(&captured.preparation.workspace, &thread)
                    .map(|facts| crate::fact_control::unsettled_mutation_paths(&facts))
                    .unwrap_or_default();
            if let Some(directive) =
                owed_work_directive(&owed, &captured.config.verification_contracts)
            {
                prompt = format!("{prompt}\n\n{directive}");
            }
        }
        // Exact reservation precedes capability admission, as in the original
        // ingress. Existing is a passive replay and cannot mint another owner.
        let external = if let Some(id) = exact.as_deref() {
            let reservation = match captured.controls.reserve_run_with_id(id, &prompt).await {
                Ok(actual) => actual,
                Err(error @ CodeError::RunIdentityConflict { .. }) => {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    return Err(error);
                }
                Err(error) => return Err(error), // Unknown Created/owner failure remains unproved.
            };
            if reservation.replayed() {
                original.record_scope_not_attempted(Scope::Fact)?;
                let snapshot = reservation.snapshot().clone();
                owned_no_run_finish(&captured, &original).await?;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(OwnedReady::Replayed(snapshot)));
                }
                return Ok(None);
            }
            let actual = original.created_handle()?;
            if bind_source_generation {
                if let Some(binding) = checkpoint
                    .as_ref()
                    .and_then(|value| value.capability_binding.clone())
                {
                    captured
                        .controls
                        .bind_capability_generation(id, binding)
                        .await?;
                }
            }
            Some(actual)
        } else {
            None
        };
        let event_tx = if mode == run_admission::ExecutionMode::Blocking {
            None
        } else {
            Some(events.clone())
        };
        let mut pinned =
            prepare_owned_fact(captured, Arc::clone(&original), &prompt, external, event_tx)
                .await?;
        if let Some(surface) = pinned.parts.surface.as_mut() {
            surface.goal_tick = goal_tick;
        }
        if let Some(checkpoint) = checkpoint.as_ref() {
            let seeded = seed_resumed_mutation_parts(&pinned.parts, checkpoint).and_then(|()| {
                if ordinary {
                    Ok(())
                } else {
                    seed_exact_recovery_parts(&pinned.parts, &checkpoint.messages)
                }
            });
            if let Err(error) = seeded {
                return Err(
                    fail_owned_prepared(pinned, CodeError::Session(format!("{error:#}"))).await,
                );
            }
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(OwnedReady::Fact {
                run: pinned.run.clone(),
                snapshot: pinned.snapshot.clone(),
            }));
        }
        settle_owned_fact(
            pinned,
            OwnedFactExecution {
                prompt,
                attachments,
                history,
                checkpoint,
                ordinary,
                exact,
                mode,
            },
            events,
        )
        .await
    }
    .await;
    match resolved {
        Ok(Some(answer)) => {
            let _ = result.send(Ok(answer));
        }
        Ok(None) => {}
        Err(error) => {
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(error));
            } else {
                let _ = result.send(Err(error));
            }
        }
    }
}

pub(super) struct OwnedFactExecution {
    pub(super) prompt: String,
    pub(super) attachments: Vec<Attachment>,
    pub(super) history: Option<Vec<Message>>,
    pub(super) checkpoint: Option<LoopCheckpoint>,
    pub(super) ordinary: bool,
    pub(super) exact: Option<String>,
    pub(super) mode: run_admission::ExecutionMode,
}

pub(super) async fn settle_owned_fact(
    mut pinned: OwnedPinnedFact,
    execution: OwnedFactExecution,
    events: mpsc::Sender<AgentEvent>,
) -> Result<Option<AgentResult>> {
    let original = Arc::clone(&pinned.original);
    let host_cancellation = pinned
        .parts
        .surface
        .as_ref()
        .map(|surface| surface.cancel.clone());
    let OwnedFactExecution {
        prompt,
        attachments,
        history,
        mut checkpoint,
        ordinary,
        exact,
        mode,
    } = execution;
    let mut current_goal = None;
    let settled = async {
        let fact = pinned.parts.open()?;
        current_goal = fact.current_goal().cloned();
        if ordinary {
            if let Some(checkpoint) = checkpoint.as_ref() {
                let mut seed = checkpoint.messages.clone();
                if seed
                    .last()
                    .is_some_and(|message| message.role == "assistant")
                {
                    seed.pop();
                }
                fact.seed_recovery_history(&seed, &prompt)?;
            }
            fact.resume_limit(32).await
        } else if checkpoint.is_some() {
            fact.resume_limit(32).await
        } else {
            if let Some(history) = history.as_deref() {
                fact.seed_history(history)?;
            }
            prepare_pre_analysis(&pinned.parts, &prompt, fact.current_goal()).await?;
            let settled = fact
                .user_text_with_attachments(&prompt, &attachments)
                .await?;
            settle_confirmations(&fact, settled, &pinned.parts).await
        }
    }
    .await;
    let mut usage = pinned.parts.usage();
    let messages = pinned.parts.transcript();
    if ordinary {
        if let Some(checkpoint) = checkpoint.as_ref() {
            usage.prompt_tokens = usage
                .prompt_tokens
                .saturating_add(checkpoint.total_usage.prompt_tokens);
            usage.completion_tokens = usage
                .completion_tokens
                .saturating_add(checkpoint.total_usage.completion_tokens);
            usage.total_tokens = usage
                .total_tokens
                .saturating_add(checkpoint.total_usage.total_tokens);
        }
    }
    let mut mapped = settled
        .as_ref()
        .map(|settled| agent_result(settled, messages, usage))
        .map_err(|error| anyhow::anyhow!("{error:#}"));
    if ordinary {
        if let (Ok(result), Some(checkpoint)) = (&mut mapped, checkpoint.take()) {
            result.tool_calls_count = result
                .tool_calls_count
                .saturating_add(checkpoint.tool_calls_count);
        }
    }
    // Capture turn-local evidence before terminal Steer adds another user fact.
    if let Ok(result) = &mut mapped {
        result.forced_stop = captured_forced_stop(&pinned.parts);
    }
    let mut accepted_step = None;
    let mut goal_attestation = None;
    if let (Some(surface), Some(goal), Ok(result)) =
        (&pinned.parts.surface, &current_goal, &mut mapped)
    {
        let ledger = surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut fresh = Vec::new();
        surface.agent.merge_completion_attestor(&ledger, &mut fresh);
        match crate::fact_control::assess_goal_step(surface, goal, result, fresh.first()).await {
            Ok((prepared, usage)) => {
                accepted_step = prepared;
                result.usage.accumulate(&usage);
            }
            Err(error) => mapped = Err(error),
        }
        goal_attestation = Some(fresh);
    }
    mapped = settle_owned_capability_run(&pinned, mapped).await;
    pinned.parts.retire_fact_host_events();
    pinned.parts.retire_checkpoint_senders();
    // A late checkpoint cannot overwrite the terminal retained input/history.
    let committer = observe_owned_committer(&original).await;
    if let Err(error) = committer {
        mapped = combine_retention_failure(mapped, "checkpoint committer", error);
    } else if let Some(surface) = &pinned.parts.surface {
        if let Err(error) = crate::fact_control::settle_terminal_run_controls(surface).await {
            mapped = combine_retention_failure(mapped, "terminal input retention", error);
        }
        match crate::fact_control::retained_transcript(surface) {
            Ok(messages) => {
                if let Ok(result) = &mut mapped {
                    result.messages = messages;
                }
            }
            Err(error) => {
                mapped = combine_retention_failure(mapped, "terminal transcript", error);
            }
        }
    }
    original.seal_original_controls().await?;
    mapped =
        mapped.and_then(|result| gate_result(&pinned.parts, result, goal_attestation.as_deref()));
    if mapped.is_ok() {
        if let (Some(surface), Some(prepared)) = (&pinned.parts.surface, accepted_step) {
            if let Err(error) = crate::fact_control::commit_goal_step(
                surface,
                prepared,
                &original.model_call_port(),
            ) {
                mapped = Err(error);
            }
        }
    }
    if let (Some(surface), Some(goal)) = (&pinned.parts.surface, &current_goal) {
        let usage = mapped.as_ref().ok().map(|result| result.usage.clone());
        if let Err(error) = crate::fact_control::settle_goal_slice(
            surface,
            goal,
            usage,
            &original.model_call_port(),
        ) {
            mapped = combine_retention_failure(mapped, "goal slice settlement", error);
        }
    }
    let update_history = history.is_none() && !ordinary && exact.is_none();
    // Keep each original route's memory/End/committer ordering.
    let admitted = if ordinary {
        None
    } else {
        admit_settled_memory(&pinned.parts, &prompt, &mapped)
    };
    // The completed fact can no longer open another main model facade. Drop
    // its strong host senders before telemetry or a retained memory tail can
    // clone the original agent; the passive channel identity remains bound.
    pinned.parts.retire_fact_host_events();
    let publication = if mode == run_admission::ExecutionMode::Blocking && !ordinary {
        if let Some(ticket) = admitted {
            extract_admitted_memory(&pinned.parts, &prompt, &mapped, ticket).await;
        }
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, update_history)
            .await?;
        drop(pinned.parts);
        publication
    } else if mode == run_admission::ExecutionMode::Streaming {
        let streamed = pinned.parts.surface.as_ref().is_some_and(|surface| {
            surface
                .streamed_text
                .load(std::sync::atomic::Ordering::Relaxed)
        });
        if let Some(surface) = &mut pinned.parts.surface {
            surface.events.take();
            surface.checkpoint.take();
        }
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, update_history)
            .await?;
        let telemetry =
            emit_memory_telemetry(&events, &pinned.parts, host_cancellation.as_ref()).await;
        spawn_recalled_memory_usage(&pinned.parts, telemetry, &mapped);
        emit_gated(
            &events,
            &settled,
            &mapped,
            streamed,
            publication,
            pinned.run.id(),
            host_cancellation.as_ref(),
        )
        .await;
        drop(events);
        let parts = pinned.parts;
        tokio::spawn(async move {
            if let Some(ticket) = admitted {
                extract_admitted_memory(&parts, &prompt, &mapped, ticket).await;
            }
        });
        pinned.cleanup.finish_owned(&original).await?;
        return Ok(None);
    } else {
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, false)
            .await?;
        if mode != run_admission::ExecutionMode::Blocking {
            let streamed = pinned.parts.surface.as_ref().is_some_and(|surface| {
                surface
                    .streamed_text
                    .load(std::sync::atomic::Ordering::Relaxed)
            });
            emit_gated(
                &events,
                &settled,
                &mapped,
                streamed,
                publication,
                pinned.run.id(),
                host_cancellation.as_ref(),
            )
            .await;
        }
        drop(events);
        if let Some(surface) = &mut pinned.parts.surface {
            surface.events.take();
        }
        if let Some(ticket) = admitted {
            extract_admitted_memory(&pinned.parts, &prompt, &mapped, ticket).await;
        }
        drop(pinned.parts);
        publication
    };
    if let Err(cleanup) = pinned.cleanup.finish_owned(&original).await {
        return Err(match mapped {
            Ok(_) => cleanup,
            Err(primary) => CodeError::Session(format!(
                "{primary:#}; original cleanup remains unproved: {cleanup}"
            )),
        });
    }
    if publication == run_admission::OriginalPublicationKind::Cancelled {
        return Err(CodeError::run_cancelled(pinned.run.id().to_owned()));
    }
    let mut answer = mapped.map_err(|error| CodeError::Session(format!("{error:#}")))?;
    if update_history {
        strip_attachment_annotation(&mut answer.messages);
    }
    Ok(Some(answer))
}
