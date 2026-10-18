//! Model calls for one fact-log turn: governed response, framing, waits, and usage.

use super::*;

pub(super) fn interrupted_response() -> crate::llm::LlmResponse {
    crate::llm::LlmResponse {
        message: Message::assistant("(Response interrupted by the user.)"),
        usage: crate::llm::TokenUsage::default(),
        stop_reason: Some("cancelled".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

pub(super) async fn complete_detached(
    client: Arc<dyn LlmClient>,
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
    cancel: tokio_util::sync::CancellationToken,
) -> Result<crate::llm::LlmResponse, ActorError> {
    if cancel.is_cancelled() {
        return Ok(interrupted_response());
    }
    let attempt = cancel.child_token();
    match client
        .complete_streaming(messages, system, tools, attempt.clone())
        .await
    {
        Ok(mut events) => {
            let mut done = None;
            loop {
                let event = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Ok(interrupted_response()),
                    _ = attempt.cancelled() => {
                        return Err(ActorError::Handler {
                            key: "infer".into(),
                            message: "LLM call timed out".into(),
                        });
                    }
                    event = events.recv() => match event {
                        Some(event) => event,
                        None => break,
                    },
                };
                if let crate::llm::StreamEvent::Done(response) = event {
                    done = Some(response);
                }
            }
            if let Some(response) = done {
                return Ok(response);
            }
            if cancel.is_cancelled() {
                return Ok(interrupted_response());
            }
            client
                .complete(messages, system, tools)
                .await
                .map_err(model_error)
        }
        Err(_) => client
            .complete(messages, system, tools)
            .await
            .map_err(model_error),
    }
}

pub(crate) fn model_error(error: anyhow::Error) -> ActorError {
    // Host admission refusal is terminal for this attempt. Preserve the
    // existing fold policy: only the typed denial uses its non-retryable lane.
    if let Some(denial @ crate::error::CodeError::BudgetExhausted { .. }) =
        error.downcast_ref::<crate::error::CodeError>()
    {
        return ActorError::Defect(denial.to_string());
    }
    if let Some(message) = crate::llm::non_retryable_llm_error_message(&error) {
        return ActorError::Defect(message.to_string());
    }
    ActorError::Handler {
        key: "infer".into(),
        message: error.to_string(),
    }
}

pub(super) async fn model_response(
    client: Arc<dyn LlmClient>,
    messages: Vec<Message>,
    system: Option<String>,
    tools: Vec<ToolDefinition>,
    surface: Option<SessionSurface>,
    host_cancel: tokio_util::sync::CancellationToken,
    original_model: Option<crate::agent_api::OriginalModelCallPort>,
) -> Result<crate::llm::LlmResponse, ActorError> {
    if let Some(original) = original_model {
        let surface = surface.as_ref().ok_or_else(|| {
            ActorError::Defect("original model requires its original parent surface".into())
        })?;
        return governed_model_response(client, messages, system, tools, surface, original).await;
    }
    let Some(surface) = surface else {
        return complete_detached(client, &messages, system.as_deref(), &tools, host_cancel).await;
    };
    // A child run's scoped client admits each call against the parent
    // session's budget.
    if !surface.agent.is_child_run() {
        surface
            .agent
            .fact_budget_gate(&surface.session_id, &surface.cancel)
            .await
            .map_err(|error| ActorError::Defect(error.to_string()))?;
    }
    if surface.cancel.is_cancelled() {
        return Ok(interrupted_response());
    }
    // The provider read runs on its own task. A stalled socket poll must not
    // pin the deadline to that worker: aborting the task is what lets the
    // fold retry the attempt. Host cancellation aborts that task immediately
    // and is not a retryable provider failure.
    //
    // The configured API timeout is the quiet gap, not a wall clock around a
    // live stream. Each stream event starts that gap over. Silence, including
    // a non-streaming read that never returns, still aborts the worker.
    let idle = surface.agent.llm_api_timeout();
    let cancel = surface.cancel.clone();
    let attempt = surface.cancel.child_token();
    let attempt_for_task = attempt.clone();
    let activity = Arc::new(tokio::sync::Notify::new());
    let activity_for_task = Arc::clone(&activity);
    let handle = tokio::spawn(async move {
        read_model_response(
            client.as_ref(),
            &messages,
            system.as_deref(),
            &tools,
            &surface,
            &attempt_for_task,
            activity_for_task.as_ref(),
        )
        .await
    });
    // A caller may drop this future (the run deadline does); the provider
    // read must not outlive it.
    let _abort_on_drop = AbortOnDrop(handle.abort_handle());
    await_model_task_with_idle(handle, &cancel, &attempt, activity.as_ref(), idle).await
}

pub(super) async fn governed_model_response(
    client: Arc<dyn LlmClient>,
    messages: Vec<Message>,
    system: Option<String>,
    tools: Vec<ToolDefinition>,
    surface: &SessionSurface,
    original: crate::agent_api::OriginalModelCallPort,
) -> Result<LlmResponse, ActorError> {
    // This parent marker belongs to this response, including Done-only replies.
    surface.streamed_text.store(false, Ordering::Relaxed);
    // All fallible preparation, actual runtime lookup and owner checks occur
    // before the existing child is spawned. No fallback owner is constructed.
    let prepared = surface
        .agent
        .prepare_fact_model_call(&surface.session_id, client, messages, system, tools)
        .map_err(model_error)?;
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| ActorError::Defect(error.to_string()))?;
    let reservation = original
        .reserve_same_original_call(&surface.session_id)
        .await
        .map_err(|error| ActorError::Defect(error.to_string()))?;
    let attempt = surface.cancel.child_token();
    let _request_on_drop = attempt.clone().drop_guard();
    let accounting = Arc::new(crate::agent::FactModelAccountingObservation::default());
    let child_accounting = Arc::clone(&accounting);
    let child_attempt = attempt.clone();
    let (sender, mut frames) = tokio::sync::mpsc::channel(64);
    let (start, started) = tokio::sync::oneshot::channel();
    let actual = runtime.spawn(async move {
        if started.await.is_err() {
            return Err(ActorError::Defect(
                "original model start channel closed".into(),
            ));
        }
        prepared.run(sender, child_attempt, child_accounting).await
    });
    // No await between actual spawn, infallible install and original start.
    reservation.install_and_start(actual, attempt.clone(), Arc::clone(&accounting), start);

    let idle_limit = surface.agent.llm_api_timeout();
    let joined = original.wait_same_original_call();
    tokio::pin!(joined);
    let mut settled = None;
    let mut response = None;
    let mut closed = false;
    // The configured API timeout is the quiet gap between frames. A live
    // original call keeps its gap open. Dropping this parent receiver still
    // requests the same attempt; it does not abort the child.
    loop {
        if surface.cancel.is_cancelled() {
            return Ok(interrupted_response());
        }
        if closed && settled.is_some() {
            let result = settled
                .take()
                .ok_or_else(|| ActorError::Defect("missing actual model result".into()))?;
            result?;
            return response.ok_or_else(|| ActorError::Handler {
                key: "infer".into(),
                message: "stream ended before a response".into(),
            });
        }
        tokio::select! {
            biased;
            _ = surface.cancel.cancelled() => return Ok(interrupted_response()),
            frame = frames.recv(), if !closed => match frame {
                Some(frame) => {
                    if let Some(done) = project_model_frame(surface, frame, &accounting).await? {
                        response = Some(done);
                    }
                }
                None => closed = true,
            },
            result = &mut joined, if settled.is_none() => settled = Some(result),
            _ = model_idle_sleep(idle_limit) => {
                let Some(idle_limit) = idle_limit else {
                    continue;
                };
                return Err(llm_idle_timeout_error(idle_limit));
            }
        }
    }
}

pub(super) async fn project_model_frame(
    surface: &SessionSurface,
    frame: crate::agent::FactModelFrame,
    accounting: &crate::agent::FactModelAccountingObservation,
) -> Result<Option<LlmResponse>, ActorError> {
    use crate::agent::{AgentEvent, FactModelFrame};
    let mut capability_digest = None;
    let mut usage = false;
    let event = match frame {
        FactModelFrame::RawStream(crate::llm::StreamEvent::Done(response)) => {
            record_usage(surface, &response);
            return Ok(Some(response));
        }
        FactModelFrame::RawStream(crate::llm::StreamEvent::TextDelta(text)) => {
            surface.streamed_text.store(true, Ordering::Relaxed);
            AgentEvent::TextDelta { text }
        }
        FactModelFrame::RawStream(crate::llm::StreamEvent::ReasoningDelta(text))
            if !text.is_empty() =>
        {
            AgentEvent::ReasoningDelta { text }
        }
        FactModelFrame::RawStream(_) => return Ok(None),
        FactModelFrame::Capability {
            call_sequence,
            snapshot,
        } => {
            if surface
                .agent
                .fact_capability_is_current(&snapshot.snapshot_digest)
                .await
            {
                return Ok(None);
            }
            capability_digest = Some(snapshot.snapshot_digest.clone());
            AgentEvent::RunCapabilityBound {
                call_sequence,
                snapshot,
            }
        }
        FactModelFrame::Presentation(snapshot) => AgentEvent::ModelPresentationBound { snapshot },
        FactModelFrame::Input(snapshot) => AgentEvent::ModelInputBound { snapshot },
        FactModelFrame::Usage(snapshot) => {
            usage = true;
            AgentEvent::ModelUsageBound { snapshot }
        }
        FactModelFrame::Threshold(crate::budget::BudgetDecision::SoftLimit {
            resource,
            consumed,
            limit,
            message,
        }) => AgentEvent::BudgetThresholdHit {
            resource,
            kind: "soft".into(),
            consumed,
            limit,
            message,
        },
        FactModelFrame::Threshold(crate::budget::BudgetDecision::Deny { resource, reason }) => {
            AgentEvent::BudgetThresholdHit {
                resource,
                kind: "hard".into(),
                consumed: 0.0,
                limit: 0.0,
                message: Some(reason),
            }
        }
        FactModelFrame::Threshold(crate::budget::BudgetDecision::Allow) => return Ok(None),
    };
    let observed = if usage {
        &accounting.usage_projection
    } else {
        &accounting.last_projection
    };
    {
        let mut state = observed
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?;
        *state = crate::agent::FactProjectionObservation {
            _queued: true,
            ..Default::default()
        };
    }
    record_run_event_observed(surface, event, observed).await?;
    {
        let state = observed
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?;
        if !matches!(
            &state.store,
            crate::agent::FactStoreObservation::Returned(Some(_))
        ) {
            return Err(ActorError::Defect(
                "original model RunStore projection did not return Some".into(),
            ));
        }
        if matches!(
            &state.output,
            crate::agent::FactOutputObservation::Returned(Err(_))
        ) {
            return Err(ActorError::Defect(
                "original model public observation receiver closed".into(),
            ));
        }
    }
    if let Some(digest) = capability_digest {
        surface.agent.fact_capability_projected(digest).await;
    }
    Ok(None)
}

pub(super) async fn record_run_event_observed(
    surface: &SessionSurface,
    event: crate::agent::AgentEvent,
    observation: &Mutex<crate::agent::FactProjectionObservation>,
) -> Result<(), ActorError> {
    use crate::agent::{FactOutputObservation, FactStoreObservation};
    if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .store = FactStoreObservation::Entered;
        let returned = store.record_event(run_id, event.clone()).await;
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .store = FactStoreObservation::Returned(returned.map(Box::new));
    } else {
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .store = FactStoreObservation::NotConfigured;
    }
    if let Some(sender) = &surface.events {
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .output = FactOutputObservation::Entered;
        let returned = sender.send(event).await;
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .output = FactOutputObservation::Returned(returned.map_err(Box::new));
    } else {
        observation
            .lock()
            .map_err(|_| ActorError::Defect("poisoned model projection observation".into()))?
            .output = FactOutputObservation::NotConfigured;
    }
    Ok(())
}

pub(super) async fn read_model_response(
    client: &dyn LlmClient,
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
    surface: &SessionSurface,
    attempt: &tokio_util::sync::CancellationToken,
    activity: &tokio::sync::Notify,
) -> Result<crate::llm::LlmResponse, ActorError> {
    let (evidence_events, usage_binding) = surface
        .agent
        .fact_model_evidence(messages, system, tools)
        .await;
    for event in evidence_events {
        record_run_event(surface, event).await;
    }
    // A child run nobody listens to does not stream: a refused stream setup
    // would admit the same call twice through the fallback.
    if surface.agent.is_child_run() && surface.events.is_none() {
        let response = client
            .complete(messages, system, tools)
            .await
            .map_err(model_error)?;
        record_usage(surface, &response);
        record_model_usage_event(surface, usage_binding.as_ref(), &response).await;
        return Ok(response);
    }
    match client
        .complete_streaming(messages, system, tools, attempt.clone())
        .await
    {
        Ok(mut events) => {
            // Per-response, not per-turn: each model response streams on its
            // own. A final response that produced no deltas (non-streaming
            // fallback, an empty finish) must be re-sent at settle even when
            // an earlier response in the same turn streamed — the old
            // turn-level flag suppressed the re-send here while the host's
            // turn-level de-dup guard dropped the End text, losing the final
            // answer on both rails at once.
            surface
                .streamed_text
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let mut done = None;
            loop {
                let event = tokio::select! {
                    biased;
                    // Host cancellation is the parent of the attempt token, so
                    // it must win over the deadline branch.
                    _ = surface.cancel.cancelled() => return Ok(interrupted_response()),
                    _ = attempt.cancelled() => {
                        return Err(ActorError::Handler {
                            key: "infer".into(),
                            message: "LLM call timed out".into(),
                        });
                    }
                    event = events.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                // Any frame, including an empty reasoning heartbeat, is proof
                // the provider is still sending. The outer quiet gap starts over.
                activity.notify_one();
                match event {
                    crate::llm::StreamEvent::TextDelta(text) => {
                        // This is the streaming path to the host: forward each
                        // delta and mark the surface so the settled replay does
                        // not re-send the full assistant text.
                        surface
                            .streamed_text
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        let event = crate::agent::AgentEvent::TextDelta { text };
                        if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
                            store.record_event(run_id, event.clone()).await;
                        }
                        if let Some(sender) = &surface.events {
                            let _ = sender.send(event).await;
                        }
                    }
                    crate::llm::StreamEvent::ReasoningDelta(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        let event = crate::agent::AgentEvent::ReasoningDelta { text };
                        if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
                            store.record_event(run_id, event.clone()).await;
                        }
                        if let Some(sender) = &surface.events {
                            let _ = sender.send(event).await;
                        }
                    }
                    crate::llm::StreamEvent::Done(response) => done = Some(response),
                    _ => {}
                }
            }
            if let Some(response) = done {
                record_usage(surface, &response);
                record_model_usage_event(surface, usage_binding.as_ref(), &response).await;
                return Ok(response);
            }
            if surface.cancel.is_cancelled() || attempt.is_cancelled() {
                return Ok(interrupted_response());
            }
            Err(ActorError::Handler {
                key: "infer".into(),
                message: "stream ended before a response".into(),
            })
        }
        Err(error) => {
            if surface.cancel.is_cancelled() {
                return Ok(interrupted_response());
            }
            let failure = model_error(error);
            if matches!(failure, ActorError::Defect(_)) {
                return Err(failure);
            }
            let response = client
                .complete(messages, system, tools)
                .await
                .map_err(model_error)?;
            record_usage(surface, &response);
            record_model_usage_event(surface, usage_binding.as_ref(), &response).await;
            Ok(response)
        }
    }
}

pub(super) async fn record_run_event(surface: &SessionSurface, event: crate::agent::AgentEvent) {
    // Preserve old unit callers through the one actual store/public writer.
    let observation = Mutex::new(crate::agent::FactProjectionObservation::default());
    let _ = record_run_event_observed(surface, event, &observation).await;
}

pub(super) async fn record_model_usage_event(
    surface: &SessionSurface,
    binding: Option<&crate::harness_evidence::ModelUsageBinding>,
    response: &crate::llm::LlmResponse,
) {
    let Some(binding) = binding else {
        return;
    };
    let Some(event) = crate::agent::AgentLoop::fact_model_usage_event(binding, &response.usage)
    else {
        return;
    };
    record_run_event(surface, event).await;
}

pub(super) fn record_usage(surface: &SessionSurface, response: &crate::llm::LlmResponse) {
    let mut usage = response.usage.clone();
    // Some providers return the assistant text and omit the usage object.
    // A real turn still has to move the carried-forward totals.
    if usage.total_tokens == 0 {
        let reply = response.text();
        if !reply.is_empty() {
            let estimated = reply.len().div_ceil(4).max(1);
            usage.completion_tokens = usage.completion_tokens.max(estimated);
            usage.total_tokens = estimated;
        }
    }
    surface
        .usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .accumulate(&usage);
}

/// Bound context setup so a stalled provider cannot consume the whole turn
/// before the model is asked for a tool call. A timeout keeps the original
/// prompt and continues. A real context error still fails the attempt.
pub(super) async fn model_context(
    surface: &SessionSurface,
    prompt: &str,
    message_count: usize,
) -> Result<(String, Option<String>), ActorError> {
    let work = surface
        .agent
        .fact_model_context(prompt, &surface.session_id, message_count);
    let assembled = match surface.agent.llm_api_timeout() {
        None => work
            .await
            .map_err(|error| ActorError::Defect(error.to_string()))?,
        Some(timeout) => {
            let limit = timeout.min(std::time::Duration::from_secs(20));
            match tokio::time::timeout(limit, work).await {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => return Err(ActorError::Defect(error.to_string())),
                Err(_) => {
                    return Ok((prompt.to_string(), None));
                }
            }
        }
    };
    // Park the recall telemetry where the stream worker can emit it before
    // End — the actor itself never sends into the host channel.
    if !assembled.recalled_memories.is_empty() {
        surface
            .pending_memory_telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(MemoryTelemetryBatch {
                query: prompt.to_string(),
                items: assembled.recalled_memories,
            });
    }
    append_context_sections_fact(surface, &assembled.section_digests);
    Ok((assembled.effective_prompt, assembled.augmented_system))
}
