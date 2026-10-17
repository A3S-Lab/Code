use super::*;

pub(super) struct OwnedPinnedFact {
    pub(super) run: crate::run::RunHandle,
    pub(super) snapshot: crate::run::RunSnapshot,
    pub(super) parts: FactSession,
    pub(super) guard: crate::capability::SessionCapabilityRun,
    pub(super) finish: TurnFinish,
    pub(super) cleanup: super::super::runtime_events::RunCleanupState,
    pub(super) controls: RunControlState,
    pub(super) original: Arc<run_admission::OriginalExecutionContext>,
}

// Preparation uses only concrete values captured before the start-gated
// worker. Actual capability and Run returns are recorded before the next await.
pub(super) async fn prepare_owned_fact(
    captured: OwnedConversationCapture,
    original: Arc<run_admission::OriginalExecutionContext>,
    prompt: &str,
    external: Option<crate::run::RunHandle>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
) -> Result<OwnedPinnedFact> {
    use run_admission::OriginalScopeKind as Scope;
    let OwnedConversationCapture {
        preparation,
        coordinator,
        controls,
        cleanup,
        finish,
        mut parts,
        config,
        recorded_reports,
        ..
    } = captured;
    let super::super::agent_loop_runtime::OwnedPreparationCapture {
        fact,
        construction,
        checkpoint,
        cognitive_binding,
        cancellation,
        session_id,
        boundary,
        ..
    } = preparation;
    let cleanup = match external.as_ref() {
        Some(actual) => cleanup.with_run_id(actual.id().to_owned()),
        None => cleanup,
    };
    original.mark_scope_attempted(Scope::Fact)?;
    let (mut agent, guard) = match super::super::agent_loop_runtime::build_owned_pinned_agent_loop(
        fact,
        construction,
        boundary,
        cancellation.clone(),
    )
    .await
    {
        Ok(actual) => actual,
        Err(failure) => {
            let (error, actual) = failure.into_original_observation();
            if let Some((guard, close)) = actual {
                original.record_scope_returned(Scope::Fact, &guard)?;
                original.record_scope_close(Scope::Fact, &close)?;
            }
            // No Run/inbox/committer construction was reached on this branch.
            if external.is_none() {
                original.record_no_run_branch()?;
            }
            original.record_no_inbox()?;
            original.record_no_committer()?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    original.record_scope_returned(Scope::Fact, &guard)?;
    let binding = match agent.checkpoint_capability_binding().cloned() {
        Some(binding) => binding,
        None => {
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        }
    };
    let run_result = if let Some(run) = external {
        let id = run.id().to_owned();
        async {
            controls
                .bind_capability_generation(&id, binding.clone())
                .await?;
            if let Some(value) = cognitive_binding.clone() {
                controls.bind_cognitive_package(&id, value).await?;
            }
            Ok(run)
        }
        .await
    } else {
        controls
            .start_run_with_bindings(prompt, cognitive_binding.clone(), binding.clone())
            .await
    };
    let run = match run_result {
        Ok(run) => run,
        Err(error) => {
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            // reserve may already have returned Created; absence is not proof.
            // T005 preserves any unknown binding and refuses false release.
            original.record_no_inbox()?;
            original.record_no_committer()?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    let run_id = run.id().to_owned();
    let cleanup = cleanup.with_run_id(run_id.clone());
    if let Some(binding) = cognitive_binding {
        finish
            .run_store
            .record_event(&run_id, AgentEvent::CognitiveContextBound { binding })
            .await;
    }
    let started = AgentEvent::Start {
        prompt: prompt.to_owned(),
    };
    finish
        .run_store
        .record_event(&run_id, started.clone())
        .await;
    finish.note(&run_id, &started).await;
    let (sink, mut checkpoints) =
        super::super::runtime_checkpoints::runtime_checkpoint_channel_owned(
            checkpoint.store,
            checkpoint.export,
            checkpoint.persistence,
        );
    let checkpoint = match sink {
        Some(sink) => {
            let install = original.reserve_committer_install()?;
            let actual = tokio::spawn(async move {
                while let Some(boundary) = checkpoints.recv().await {
                    checkpoints.commit(boundary).await;
                }
            });
            install.install(actual); // No await before the actual handle is retained.
            Some(crate::fact_control::CheckpointEmit {
                sink,
                run_id: run_id.clone(),
                session_id: session_id.clone(),
                capability_binding: Some(binding),
            })
        }
        None => {
            drop(checkpoints);
            original.record_no_committer()?;
            None
        }
    };
    let prepared = match ExecutionCoordinator::prepare_owned(
        coordinator,
        controls.clone(),
        &original,
        run_id.clone(),
        &mut agent,
        cancellation.clone(),
    )
    .await
    {
        Ok(actual) => actual,
        Err(error) => {
            drop(checkpoint);
            drop(agent);
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            observe_owned_committer(&original).await?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    prepared.run_control().update_turn(1).await;
    let (lane, _) = tokio::sync::broadcast::channel::<AgentEvent>(1024);
    agent = agent.with_agent_event_tx(lane);
    // The fact actor relays the broadcast lane per tool call. There is no
    // actual barrier receiver here. Select the final runtime lane before
    // creating/binding the one invocation and its matched governed client.
    let invocation = prepared.fact_invocation(&agent, event_tx.clone());
    agent = invocation.bind_agent_loop(&agent);
    parts.client =
        Some(agent.scoped_llm_client_for_parts(Some(&session_id), &event_tx, &cancellation));
    parts.executor = agent.tool_executor_handle();
    parts.original_model = Some(original.model_call_port());
    parts.surface = Some(crate::fact_control::SessionSurface {
        agent,
        session_id,
        checkpoint,
        events: event_tx,
        pending_memory_telemetry: Arc::new(std::sync::Mutex::new(Vec::new())),
        fact_log: Arc::clone(&parts.fact_log),
        streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        forced_stop: Arc::new(std::sync::Mutex::new(None)),
        cancel: cancellation,
        transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
        usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: config.confirmation_manager,
        run_store: Some(Arc::clone(&finish.run_store)),
        run_id: Some(run_id.clone()),
        goal_tick: None,
        ledger: Arc::new(std::sync::Mutex::new(
            crate::harness_loop::MutationLedger::default(),
        )),
        reports: Arc::new(std::sync::Mutex::new(recorded_reports)),
        run_control: Some(prepared.run_control()),
        targeted_paths: Arc::new(std::sync::Mutex::new(Vec::new())),
        harness: config.harness,
        host_harness_registry: config.host_harness_registry,
        host_harness_assembler: config.host_harness_assembler,
    });
    if let Some(surface) = &parts.surface {
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    let snapshot = match controls.snapshot(&run_id).await {
        Some(actual) => actual,
        None => {
            return Err(fail_owned_components(
                parts,
                guard,
                controls,
                cleanup,
                original,
                CodeError::Session(format!(
                    "bound exact Run '{run_id}' disappeared before execution"
                )),
            )
            .await)
        }
    };
    Ok(OwnedPinnedFact {
        run,
        snapshot,
        parts,
        guard,
        finish,
        cleanup,
        controls,
        original,
    })
}

impl TurnFinish {
    pub(super) async fn publish_owned(
        &self,
        original: &Arc<run_admission::OriginalExecutionContext>,
        outcome: &std::result::Result<AgentResult, anyhow::Error>,
        update_history: bool,
    ) -> Result<run_admission::OriginalPublicationKind> {
        use run_admission::{
            OriginalHookObservation as Hook, OriginalPublicationKind as Kind,
            OriginalPublicationObservation, OriginalStoreObservation as Store,
        };
        let sealed = original.seal_publication()?;
        let target = original.publication_target(&sealed)?;
        let mut actual = OriginalPublicationObservation {
            kind: if target.run_id.is_none() {
                Kind::NoRun
            } else if outcome.is_err() {
                Kind::Error
            } else if target.cancelled {
                Kind::Cancelled
            } else {
                Kind::End
            },
            terminal: Store::NotApplicable,
            diagnostic: Store::NotApplicable,
            cancellation_hook: Hook::NotApplicable,
            event_hook: Hook::NotApplicable,
        };
        if let Some(run_id) = target.run_id.as_deref() {
            actual.terminal = Store::NotAttempted;
            if target.cancelled && outcome.is_err() {
                actual.diagnostic = Store::NotAttempted;
            }
            if target.cancel_origin.is_some() && self.hook.is_some() {
                actual.cancellation_hook = Hook::NotAttempted;
            }
            if self.hook.is_some() && (!target.cancelled || outcome.is_err()) {
                actual.event_hook = Hook::NotAttempted;
            }
            original.record_publication_progress(&sealed, &actual)?;
            if target.cancelled {
                actual.terminal = Store::Entered;
                original.record_publication_progress(&sealed, &actual)?;
                actual.terminal = Store::Returned(Box::new(
                    self.run_store
                        .settle_terminal(run_id, crate::run::RunTerminalTransition::Cancelled)
                        .await,
                ));
                original.record_publication_progress(&sealed, &actual)?;
                if target.cancel_origin.is_some() {
                    if let Some(hook) = &self.hook {
                        actual.cancellation_hook = Hook::Entered;
                        original.record_publication_progress(&sealed, &actual)?;
                        hook.record_run_cancelled(
                            run_id,
                            &target.session_id,
                            Some("cancelled original Run"),
                        )
                        .await;
                        actual.cancellation_hook = Hook::Returned;
                        original.record_publication_progress(&sealed, &actual)?;
                    }
                }
            }
            let event = match outcome {
                Ok(_) if target.cancelled => None,
                Ok(result) => Some(AgentEvent::End {
                    text: result.text.clone(),
                    usage: result.usage.clone(),
                    verification_summary: Box::new(
                        crate::verification::VerificationSummary::from_reports(
                            &result.verification_reports,
                        ),
                    ),
                    completion_terminal: result.completion.clone(),
                    forced_stop: result.forced_stop,
                    meta: None,
                }),
                Err(error) => Some(AgentEvent::Error {
                    message: error.to_string(),
                }),
            };
            if let Some(event) = event {
                if target.cancelled {
                    actual.diagnostic = Store::Entered;
                } else {
                    actual.terminal = Store::Entered;
                }
                original.record_publication_progress(&sealed, &actual)?;
                let returned = self.run_store.record_event(run_id, event.clone()).await;
                if target.cancelled {
                    actual.diagnostic = Store::Returned(Box::new(returned));
                } else {
                    actual.terminal = Store::Returned(Box::new(returned));
                }
                original.record_publication_progress(&sealed, &actual)?;
                if let Some(hook) = &self.hook {
                    actual.event_hook = Hook::Entered;
                    original.record_publication_progress(&sealed, &actual)?;
                    hook.record_agent_event(&event, run_id, &target.session_id)
                        .await;
                    actual.event_hook = Hook::Returned;
                    original.record_publication_progress(&sealed, &actual)?;
                }
            }
            self.persistence.clear_loop_checkpoint(run_id).await;
        }
        if let Ok(result) = outcome {
            self.persistence.record_usage(&result.usage);
            if update_history && !result.messages.is_empty() {
                let mut messages = result.messages.clone();
                strip_attachment_annotation(&mut messages);
                self.persistence.record_messages(messages);
            }
            self.persistence.auto_save_if_enabled().await;
        }
        // Project the same sealed decision to the caller/public stream. A
        // later token poll could disagree with the terminal already retained.
        let kind = actual.kind;
        original.record_publication(sealed, actual)?;
        Ok(kind)
    }
}

pub(super) async fn observe_owned_committer(
    original: &Arc<run_admission::OriginalExecutionContext>,
) -> Result<()> {
    match original.wait_committer().await? {
        run_admission::ActualCommitterJoin::PositivelyDisabled
        | run_admission::ActualCommitterJoin::Joined(
            run_admission::ActualJoinObservation::Completed,
        ) => Ok(()),
        actual => Err(CodeError::Session(format!(
            "original checkpoint committer did not complete: {actual:?}"
        ))),
    }
}

pub(super) async fn fail_owned_prepared(pinned: OwnedPinnedFact, primary: CodeError) -> CodeError {
    let OwnedPinnedFact {
        parts,
        guard,
        controls,
        cleanup,
        original,
        ..
    } = pinned;
    fail_owned_components(parts, guard, controls, cleanup, original, primary).await
}

pub(super) async fn fail_owned_components(
    mut parts: FactSession,
    guard: crate::capability::SessionCapabilityRun,
    controls: RunControlState,
    cleanup: super::super::runtime_events::RunCleanupState,
    original: Arc<run_admission::OriginalExecutionContext>,
    mut primary: CodeError,
) -> CodeError {
    let closed = guard.close().await;
    if let Err(error) = &closed {
        primary = CodeError::Session(format!(
            "{primary}; capability Run cleanup also failed: {error}"
        ));
    }
    if let Err(error) = original.record_scope_close(run_admission::OriginalScopeKind::Fact, &closed)
    {
        return CodeError::Session(format!(
            "{primary}; original close observation failed: {error}"
        ));
    }
    parts.retire_fact_host_events();
    parts.retire_checkpoint_senders();
    if let Err(error) = observe_owned_committer(&original).await {
        return CodeError::Session(format!("{primary}; {error}"));
    }
    if let Some(surface) = &parts.surface {
        if let Err(error) = crate::fact_control::settle_terminal_run_controls(surface).await {
            primary = CodeError::Session(format!(
                "{primary}; terminal input retention failed: {error}"
            ));
        }
        if let Err(error) = crate::fact_control::retained_transcript(surface) {
            primary = CodeError::Session(format!("{primary}; terminal transcript failed: {error}"));
        }
    }
    if let Err(error) = original.seal_original_controls().await {
        return CodeError::Session(format!(
            "{primary}; original controls remain unproved: {error}"
        ));
    }
    drop(parts);
    if let Err(error) = controls.fail_reserved_run_start(&original, &primary).await {
        return CodeError::Session(format!(
            "{primary}; original admission publication failed: {error}"
        ));
    }
    if let Err(error) = cleanup.finish_owned(&original).await {
        return CodeError::Session(format!(
            "{primary}; original cleanup remains unproved: {error}"
        ));
    }
    primary
}

pub(super) async fn finish_failed_admission(
    cleanup: &super::super::runtime_events::RunCleanupState,
    original: &Arc<run_admission::OriginalExecutionContext>,
    primary: CodeError,
) -> CodeError {
    if let Err(cleanup_error) = cleanup.finish_owned(original).await {
        // The original context retains its incomplete observations and lease.
        // Preserve the typed admission cause for callers; a failed cleanup
        // observation cannot replace it or authorize original-owner release.
        tracing::error!(
            primary_error = %primary,
            error = %cleanup_error,
            "Admission failed and original cleanup remains unproved"
        );
    }
    primary
}

pub(super) fn combine_original_failure(primary: CodeError, observed: Result<()>) -> CodeError {
    match observed {
        Ok(()) => primary,
        Err(error) => CodeError::Session(format!(
            "{primary}; original cleanup remains unproved: {error}"
        )),
    }
}

pub(super) async fn owned_no_run_finish(
    capture: &OwnedConversationCapture,
    original: &Arc<run_admission::OriginalExecutionContext>,
) -> Result<()> {
    original.record_no_run_branch()?;
    original.record_no_inbox()?;
    original.record_no_committer()?;
    original.seal_original_controls().await?;
    capture
        .finish
        .publish_owned(
            original,
            &Err(anyhow::anyhow!("no fact Run constructed")),
            false,
        )
        .await?;
    capture.cleanup.finish_owned(original).await
}

pub(super) async fn owned_load_checkpoint(
    store: Option<&Arc<dyn crate::store::SessionStore>>,
    session_id: &str,
    source: &str,
) -> Result<Option<LoopCheckpoint>> {
    let Some(store) = store else {
        return Ok(None);
    };
    let actual = store.load_loop_checkpoint(source).await.map_err(|error| {
        CodeError::Session(format!("load_loop_checkpoint('{source}') failed: {error}"))
    })?;
    if let Some(checkpoint) = &actual {
        checkpoint
            .ensure_owned_by(source, session_id)
            .map_err(|error| {
                CodeError::Session(format!(
                    "refusing to resume checkpoint '{source}': {error:#}"
                ))
            })?;
    }
    Ok(actual)
}
