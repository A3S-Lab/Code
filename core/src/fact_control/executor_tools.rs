//! The tool runner FactRun drives: execution, host events, and batching.

use super::*;

#[derive(Clone)]
pub(super) struct ExecutorTools {
    pub(super) executor: Arc<ToolExecutor>,
    pub(super) capability_operation: Option<Arc<crate::capability::AgentCapabilityOperation>>,
    pub(super) calls: Arc<AtomicUsize>,
    pub(super) checkpoint: Option<CheckpointState>,
    pub(super) events: Option<tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
    pub(super) context: crate::tools::ToolContext,
    pub(super) permission: PermissionPolicy,
    pub(super) checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
    pub(super) agent: Option<crate::agent::AgentLoop>,
    pub(super) run_store: Option<Arc<crate::run::InMemoryRunStore>>,
    pub(super) run_id: Option<String>,
    pub(super) ledger: Option<Arc<Mutex<crate::harness_loop::MutationLedger>>>,
    pub(super) reports: Option<Arc<Mutex<Vec<crate::verification::VerificationReport>>>>,
    /// Session-shared targeted-path log feeding path instruction injection.
    pub(super) targeted_paths: Option<Arc<Mutex<Vec<String>>>>,
    /// Set only by the preparation adapter after resolving input hooks once.
    pub(super) pre_tool_hook_resolved: bool,
    /// Per run: call digest -> (last result digest, identical results in a row).
    pub(super) repeats: Arc<Mutex<std::collections::HashMap<String, (String, u32)>>>,
}

#[derive(Clone)]
pub(super) struct CheckpointState {
    pub(super) sink: Arc<dyn crate::loop_checkpoint::LoopCheckpointSink>,
    pub(super) run_id: String,
    pub(super) session_id: String,
    pub(super) capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
    pub(super) standing_context_digest: Option<String>,
    pub(super) durable_source: Option<crate::store::SessionFactSourceV1>,
    pub(super) turns: Arc<AtomicUsize>,
}

impl ExecutorTools {
    pub(super) async fn emit(&self, event: crate::agent::AgentEvent) {
        if let (Some(store), Some(run_id)) = (&self.run_store, &self.run_id) {
            store.record_event(run_id, event.clone()).await;
        }
        if let Some(events) = &self.events {
            let _ = events.send(event).await;
        }
    }

    pub(super) async fn forward_host_events(
        &self,
        bridged: &mut Option<tokio::sync::broadcast::Receiver<crate::agent::AgentEvent>>,
    ) {
        let Some(receiver) = bridged.as_mut() else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(event) if host_bridge_event(&event) => self.emit(event).await,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }

    pub(super) async fn save_checkpoint(
        &self,
        call: &ToolCall,
        output: &str,
        is_error: bool,
        images: &[crate::llm::Attachment],
    ) -> Result<(), ActorError> {
        // The prepared route exports from committed receipts before the next
        // inference. Exporting here would precede byte observation and the
        // actor's terminal append. Keep this adapter only for legacy callers.
        if self.pre_tool_hook_resolved {
            return Ok(());
        }
        if let Some(operation) = &self.capability_operation {
            operation
                .close()
                .await
                .map_err(|error| ActorError::Defect(error.to_string()))?;
        }
        let Some(checkpoint) = &self.checkpoint else {
            return Ok(());
        };
        let turn = checkpoint.turns.fetch_add(1, Ordering::SeqCst) + 1;
        let checkpoint_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        // The durable boundary carries the mutation state settled so far.
        // A resumed surface starts fresh; without this the resumed run's
        // completion gate is blind to the mutations the interrupted run
        // already landed.
        let mutations = self
            .ledger
            .as_ref()
            .and_then(|ledger| ledger.lock().ok().map(|guard| guard.clone()))
            .unwrap_or_default();
        let reports = self
            .reports
            .as_ref()
            .and_then(|reports| reports.lock().ok().map(|guard| guard.clone()))
            .unwrap_or_default();
        checkpoint
            .sink
            .save_checkpoint(&crate::loop_checkpoint::LoopCheckpoint {
                schema_version: crate::loop_checkpoint::LoopCheckpoint::schema_for(
                    checkpoint.durable_source.as_ref(),
                ),
                run_id: checkpoint.run_id.clone(),
                session_id: checkpoint.session_id.clone(),
                capability_binding: checkpoint.capability_binding.clone(),
                standing_context_digest: checkpoint.standing_context_digest.clone(),
                durable_source: checkpoint.durable_source.clone(),
                turn,
                messages: vec![
                    Message {
                        role: "assistant".into(),
                        content: vec![crate::llm::ContentBlock::ToolUse {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            input: call.args.clone(),
                        }],
                        reasoning_content: None,
                        transcript_text: None,
                        transcript_visibility: Default::default(),
                    },
                    if images.is_empty() {
                        Message::tool_result(&call.id, output, is_error)
                    } else {
                        Message::tool_result_with_images(&call.id, output, images, is_error)
                    },
                ],
                total_usage: crate::llm::TokenUsage::default(),
                tool_calls_count: turn,
                verification_reports: reports,
                convergence: crate::loop_checkpoint::LoopConvergenceState {
                    mutations,
                    ..Default::default()
                },
                checkpoint_ms,
            })
            .await;
        Ok(())
    }
}

/// Wait for the next host-bridged event; never resolves without a receiver.
pub(super) async fn next_host_event(
    bridged: &mut Option<tokio::sync::broadcast::Receiver<crate::agent::AgentEvent>>,
) -> crate::agent::AgentEvent {
    let Some(receiver) = bridged.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        match receiver.recv().await {
            Ok(event) if host_bridge_event(&event) => return event,
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return std::future::pending().await;
            }
        }
    }
}

pub(super) fn host_bridge_event(event: &crate::agent::AgentEvent) -> bool {
    matches!(
        event,
        crate::agent::AgentEvent::SubagentStart { .. }
            | crate::agent::AgentEvent::SubagentProgress { .. }
            | crate::agent::AgentEvent::SubagentEnd { .. }
            | crate::agent::AgentEvent::TaskUpdated { .. }
            | crate::agent::AgentEvent::ConfirmationRequired { .. }
            | crate::agent::AgentEvent::ConfirmationReceived { .. }
            | crate::agent::AgentEvent::ConfirmationTimeout { .. }
            | crate::agent::AgentEvent::UserQuestion { .. }
    )
}

/// Record a tool call's file target for path instruction injection.
/// Mirrors the legacy loop's `targeted_paths` collection (`file_path` /
/// `path` argument keys); bounded to the most recent 32 targets.
pub(super) fn record_targeted_path(
    args: &serde_json::Value,
    targeted: Option<&Arc<Mutex<Vec<String>>>>,
) {
    let Some(targeted) = targeted else { return };
    let Some(path) = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|value| value.as_str())
    else {
        return;
    };
    let mut paths = targeted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if paths.iter().any(|existing| existing == path) {
        return;
    }
    paths.push(path.to_string());
    let overflow = paths.len().saturating_sub(32);
    if overflow > 0 {
        paths.drain(0..overflow);
    }
}

impl ToolRunner for ExecutorTools {
    fn prepare(
        self: Arc<Self>,
        request: a3s_effect::ToolPreparationRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<a3s_effect::PreparedToolExecution, ActorError>> {
        Box::pin(receipts::prepare_tools(self, request, cancel))
    }

    fn run(
        &self,
        call: ToolCall,
        host_cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
        let logical_turn = self.calls.fetch_add(1, Ordering::SeqCst);
        record_targeted_path(&call.args, self.targeted_paths.as_ref());
        let executor = Arc::clone(&self.executor);
        let context = self.context.clone();
        let events = self.events.clone();
        let permission = self.permission.clone();
        let checker = self.checker.clone();
        let agent = self.agent.clone();
        let checkpoint = self.checkpoint.clone();
        let run_store = self.run_store.clone();
        let run_id = self.run_id.clone();
        let ledger = self.ledger.clone();
        let reports = self.reports.clone();
        let targeted_paths = self.targeted_paths.clone();
        let pre_tool_hook_resolved = self.pre_tool_hook_resolved;
        let repeats = Arc::clone(&self.repeats);
        Box::pin(async move {
            let operation = agent
                .as_ref()
                .map(|agent| {
                    agent.begin_capability_operation(
                        logical_turn,
                        &host_cancel,
                        "fact Tool invocation",
                    )
                })
                .transpose()
                .map_err(|error| ActorError::Defect(error.to_string()))?
                .map(Arc::new);
            let context = match &operation {
                Some(operation) => operation.bind_tool_context(context),
                None => context,
            };
            let result = async {
            let tools = ExecutorTools {
                capability_operation: operation.clone(),
                executor,
                calls: Arc::new(AtomicUsize::new(0)),
                checkpoint,
                events,
                context,
                permission,
                checker,
                agent,
                run_store,
                run_id,
                ledger,
                reports,
                targeted_paths,
                pre_tool_hook_resolved,
                repeats,
            };
            emit_tool_request_bound(&tools, &call).await;
            if let Some(agent) = &tools.agent {
                if let Some((output, reason)) = skill_denial(agent, &call.name) {
                    tools
                        .emit(crate::agent::AgentEvent::PermissionDenied {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: call.args.clone(),
                            reason,
                        })
                        .await;
                    tools.save_checkpoint(&call, &output, true, &[]).await?;
                    return Ok(serde_json::Value::String(output));
                }
            }
            if execution_permission(
                &tools.permission,
                tools.checker.as_deref(),
                &call.name,
                &call.args,
            ) == PermissionDecision::Deny
            {
                let output = format!(
                    "Permission denied: Tool '{}' is blocked by permission policy.",
                    call.name
                );
                tools
                    .emit(crate::agent::AgentEvent::PermissionDenied {
                        tool_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        args: call.args.clone(),
                        reason: "Blocked by deny rule in permission policy".into(),
                    })
                    .await;
                tools.save_checkpoint(&call, &output, true, &[]).await?;
                return Ok(serde_json::Value::String(output));
            }
            tools
                .emit(crate::agent::AgentEvent::ToolStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                })
                .await;
            let mut args = call.args.clone();
            if let Some(agent) = tools.agent.as_ref().filter(|_| !tools.pre_tool_hook_resolved) {
                let session_id = tools.context.session_id.clone().unwrap_or_default();
                let decision = agent
                    .fire_pre_tool_use(&session_id, &call.name, &args, Vec::new())
                    .await;
                if let Some(denial) = decision.denial {
                    let output = format!("Hook denied {}: {}", call.name, denial.reason);
                    tools
                        .emit(crate::agent::AgentEvent::PermissionDenied {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: args.clone(),
                            reason: denial.reason,
                        })
                        .await;
                    return Ok(serde_json::Value::String(output));
                }
                if let Some(updated) = decision.updated_args {
                    args = updated;
                }
            }
            // Tool-owned escalation: a tool that declares `requires_confirmation`
            // settles through the confirmation flow even when the permission
            // checker allows it — the policy view alone cannot waive the
            // declaration. Sessions without a confirmation authority fail closed.
            if tools
                .executor
                .registry()
                .requires_confirmation(&call.name, &call.args)
            {
                let settlement: Result<(), String> = async {
                    let agent = tools.agent.as_ref().ok_or_else(|| {
                        format!(
                            "Tool '{}' requires confirmation but no confirmation authority is configured in this run.",
                            call.name
                        )
                    })?;
                    let decision = agent
                        .tool_safety_gate()
                        .confirmation_decision(&call.name, &call.args, true)
                        .await;
                    match decision {
                        crate::safety_gate::ToolGateDecision::Confirm {
                            timeout_ms,
                            timeout_action,
                        } => {
                            let manager = agent.confirmation_provider().ok_or_else(|| {
                                format!(
                                    "Tool '{}' requires confirmation but no HITL confirmation manager is configured.",
                                    call.name
                                )
                            })?;
                            let runtime = crate::tool_confirmation::ToolConfirmationRuntime::new(
                                manager.as_ref(),
                                tools.events.as_ref(),
                            );
                            // 9.1.0 moved timer settlement to the host, so an
                            // absent host would wait forever on the fact-log
                            // path. Bound the wait by the policy timeout and
                            // settle fail-closed (Reject never manufactures
                            // consent).
                            let cancellation = tools.context.cancellation_token();
                            let settlement_future = runtime.resolve_with_cancellation(
                                crate::tool_confirmation::ToolConfirmationRequest {
                                    tool_id: &call.id,
                                    tool_name: &call.name,
                                    args: &call.args,
                                    timeout_ms,
                                    timeout_action,
                                },
                                &cancellation,
                            );
                            let settle = async {
                                let timeout = std::time::Duration::from_millis(
                                    timeout_ms.max(1),
                                );
                                match tokio::time::timeout(timeout, settlement_future).await {
                                    Ok(resolution) => resolution,
                                    Err(_) => {
                                        manager.cancel(&call.id).await;
                                        crate::tool_confirmation::ToolConfirmationResolution::Rejected {
                                            output: format!(
                                                "Tool '{}' confirmation timed out; execution rejected.",
                                                call.name
                                            ),
                                        }
                                    }
                                }
                            };
                            match settle.await {
                                crate::tool_confirmation::ToolConfirmationResolution::Approved => {
                                    Ok(())
                                }
                                crate::tool_confirmation::ToolConfirmationResolution::Rejected {
                                    output,
                                } => Err(output),
                            }
                        }
                        crate::safety_gate::ToolGateDecision::Deny { output, .. } => Err(output),
                        crate::safety_gate::ToolGateDecision::Execute { .. } => Ok(()),
                    }
                }
                .await;
                if let Err(output) = settlement {
                    tools
                        .emit(crate::agent::AgentEvent::PermissionDenied {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: call.args.clone(),
                            reason: output.clone(),
                        })
                        .await;
                    tools.save_checkpoint(&call, &output, true, &[]).await?;
                    return Ok(serde_json::Value::String(output));
                }
            }
            tools
                .emit(crate::agent::AgentEvent::ToolExecutionStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: args.clone(),
                })
                .await;
            let mut bridged = tools
                .context
                .agent_event_tx
                .as_ref()
                .map(|sender| sender.subscribe());
            let cancel = tools.context.cancellation_token();
            let host_cancel = host_cancel.clone();
            let started = std::time::Instant::now();
            let execution = tools
                .executor
                .execute_with_context(&call.name, &args, &tools.context);
            tokio::pin!(execution);
            // Forward host events while the tool runs: a tool that waits on the
            // host (ask_user_form) needs its question delivered before it returns.
            let executed = loop {
                tokio::select! {
                    biased;
                    result = &mut execution => break result,
                    _ = cancel.cancelled() => {
                        let message = "cancelled".to_string();
                        tools
                            .emit(crate::agent::AgentEvent::ToolEnd {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                args: Some(args.clone()),
                                output: message.clone(),
                                exit_code: 1,
                                metadata: None,
                                error_kind: None,
                            })
                            .await;
                        return Ok(if tools.pre_tool_hook_resolved {
                            receipts::runner_interrupted_value(&message)
                        } else {
                            serde_json::Value::String(message)
                        });
                    }
                    _ = host_cancel.cancelled() => {
                        let message = "cancelled".to_string();
                        tools
                            .emit(crate::agent::AgentEvent::ToolEnd {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                args: Some(args.clone()),
                                output: message.clone(),
                                exit_code: 1,
                                metadata: None,
                                error_kind: None,
                            })
                            .await;
                        return Ok(if tools.pre_tool_hook_resolved {
                            receipts::runner_interrupted_value(&message)
                        } else {
                            serde_json::Value::String(message)
                        });
                    }
                    event = next_host_event(&mut bridged) => tools.emit(event).await,
                }
            };
            tools.forward_host_events(&mut bridged).await;
            let result = match executed {
                Ok(result) => result,
                Err(error) => {
                    let message = visible_tool_output(tools.agent.as_ref(), error.to_string());
                    tools
                        .emit(crate::agent::AgentEvent::ToolEnd {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            args: Some(call.args.clone()),
                            output: message.clone(),
                            exit_code: 1,
                            metadata: None,
                            error_kind: None,
                        })
                        .await;
                    if !tools.pre_tool_hook_resolved && workspace_boundary_error(&message) {
                        return Ok(serde_json::Value::String(message));
                    }
                    return Err(ActorError::Handler {
                        key: call.name.clone(),
                        message,
                    });
                }
            };
            let mut output = visible_tool_output(tools.agent.as_ref(), result.output);
            if let Some(agent) = &tools.agent {
                let session_id = tools.context.session_id.clone().unwrap_or_default();
                agent
                    .fire_post_tool_use(
                        &session_id,
                        &call.name,
                        &args,
                        &output,
                        result.exit_code == 0,
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    )
                    .await;
            }
            let mut metadata = result.metadata;
            if tools.pre_tool_hook_resolved {
                if let Some(ledger) = &tools.ledger {
                    receipts::observe_live_child(ledger, metadata.as_ref());
                }
            } else if let Some(agent) = &tools.agent {
                let mut bound = tools
                    .reports
                    .as_ref()
                    .map(|reports| {
                        reports
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    .unwrap_or_default();
                if let Some(ledger) = &tools.ledger {
                    let mut ledger_value = ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    agent
                        .record_fact_tool_effect(
                            &call.name,
                            &call.args,
                            result.exit_code,
                            &mut output,
                            &mut metadata,
                            &mut ledger_value,
                            &mut bound,
                        )
                        .await;
                    *ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = ledger_value;
                }
                if let Some(reports) = &tools.reports {
                    let mut current = reports
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    for report in bound {
                        if !current.contains(&report) {
                            current.push(report);
                        }
                    }
                }
            } else if let Some(ledger) = &tools.ledger {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observe_tool(&call.name, result.exit_code, metadata.as_ref());
            }
            if let Some(note) = repeated_result_note(&tools.repeats, &call.name, &call.args, &output) {
                output.push_str(&note);
            }
            tools
                .emit(crate::agent::AgentEvent::ToolEnd {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: Some(call.args.clone()),
                    output: output.clone(),
                    exit_code: result.exit_code,
                    metadata: metadata.clone(),
                    error_kind: result.error_kind.clone(),
                })
                .await;
            tools
                .save_checkpoint(&call, &output, result.exit_code != 0, &result.images)
                .await?;
            Ok(receipts::runner_result_value(
                &output,
                result.exit_code != 0,
                &result.images,
                result.exit_code,
                metadata.as_ref(),
            ))
            }.await;
            if let Some(operation) = operation {
                operation
                    .close()
                    .await
                    .map_err(|error| ActorError::Defect(error.to_string()))?;
            }
            result
        })
    }

    fn can_parallelize(&self, calls: &[ToolCall]) -> bool {
        // Hooks may redirect two disjoint write targets to the same path.
        // Read-only batches remain parallel; transformed writes keep order.
        if self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.hook_executor().is_some())
            && !calls
                .iter()
                .all(|c| tool_batch::parallel_read(&self.executor, c, calls.len()))
        {
            return false;
        }
        tool_batch::all_parallel_reads(&self.executor, calls) || batch_can_run_in_parallel(calls)
    }
}

pub(super) fn batch_can_run_in_parallel(calls: &[ToolCall]) -> bool {
    if calls.len() <= 1 {
        return false;
    }
    if calls.iter().all(|call| is_parallel_safe_read(&call.name)) {
        return true;
    }
    if !calls.iter().all(|call| is_parallel_safe_write(&call.name)) {
        return false;
    }
    // Write-class tools key their target on `file_path` (the registered
    // schema); a call without it stays sequential rather than guessing.
    let mut paths = std::collections::HashSet::new();
    calls.iter().all(|call| {
        call.args
            .get("file_path")
            .or_else(|| call.args.get("path"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|path| paths.insert(path.to_string()))
    })
}

/// Parallel-safe tool names use the REGISTERED tool names (`read`, `ls`,
/// `write`, …), not the file-operation verbs a checklist might expect. A
/// name that drifts from the registry silently turns every batch sequential.
pub(super) fn is_parallel_safe_read(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "ls"
            | "grep"
            | "glob"
            | "search"
            | "hybrid"
            | "semantic"
            | "bm25"
            | "web_search"
            | "web_fetch"
    )
}

pub(super) fn is_parallel_safe_write(name: &str) -> bool {
    matches!(name, "write" | "edit")
}

pub(super) struct NoopCompact;

impl Compactor for NoopCompact {
    fn compact(
        &self,
        messages: &[String],
    ) -> a3s_effect::coding::BoxFuture<Result<String, ActorError>> {
        let summary = bounded_summary(messages);
        Box::pin(async move { Ok(summary) })
    }
}
