use super::*;

pub(super) fn admit_settled_memory(
    parts: &FactSession,
    prompt: &str,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) -> Option<crate::memory::MemoryExtractionTicket> {
    let result = mapped.as_ref().ok()?;
    let surface = parts.surface.as_ref()?;
    surface
        .agent
        .admit_fact_memory_extraction(prompt, &result.text, &surface.cancel)
}

pub(super) async fn extract_admitted_memory(
    parts: &FactSession,
    prompt: &str,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
    ticket: crate::memory::MemoryExtractionTicket,
) {
    let Ok(result) = mapped else {
        return;
    };
    let Some(surface) = parts.surface.as_ref() else {
        return;
    };
    surface
        .agent
        .finish_admitted_fact_extraction(
            prompt,
            &result.text,
            &surface.session_id,
            &surface.cancel,
            ticket,
        )
        .await;
}

/// Drain the recall telemetry the actor parked on the surface and emit it
/// while the worker owns the host channel — always before `End`, never from
/// inside the state machine. Returns the drained batches so post-turn usage
/// telemetry can consume the same recalled set.
pub(super) async fn emit_memory_telemetry(
    tx: &mpsc::Sender<AgentEvent>,
    parts: &FactSession,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Vec<crate::fact_control::MemoryTelemetryBatch> {
    let Some(surface) = parts.surface.as_ref() else {
        return Vec::new();
    };
    let batches = std::mem::take(
        &mut *surface
            .pending_memory_telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for batch in &batches {
        let result_count = batch.items.len();
        for (memory_id, content, relevance) in batch.items.clone() {
            send_host_event(
                tx,
                AgentEvent::MemoryRecalled {
                    memory_id,
                    content,
                    relevance,
                },
                cancellation,
            )
            .await;
        }
        send_host_event(
            tx,
            AgentEvent::MemoriesSearched {
                query: Some(batch.query.clone()),
                tags: Vec::new(),
                result_count,
            },
            cancellation,
        )
        .await;
    }
    batches
}

/// Fire-and-forget lexical usage telemetry for the turn's recalled memories.
///
/// Runs on a detached task after the turn settled: store round-trips never
/// delay `End`, and the recorder itself traces every error.
pub(super) fn spawn_recalled_memory_usage(
    parts: &FactSession,
    batches: Vec<crate::fact_control::MemoryTelemetryBatch>,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) {
    let Some(surface) = parts.surface.as_ref() else {
        return;
    };
    let Ok(result) = mapped else {
        return;
    };
    let recalled: Vec<(String, String, f32)> = batches
        .iter()
        .flat_map(|batch| batch.items.iter().cloned())
        .collect();
    if recalled.is_empty() || result.text.trim().is_empty() {
        return;
    }
    let agent = surface.agent.clone();
    let final_text = result.text.clone();
    tokio::spawn(async move {
        agent
            .record_recalled_memory_usage(&recalled, &final_text)
            .await;
    });
}

// Cancellation ends host observation, never original execution ownership.
// A ready send keeps its place in the bounded FIFO; a full send is abandoned
// only after the same Run cancellation. Authoritative terminal/control events
// are retained by their caller before this best-effort host projection.
pub(super) async fn send_host_event(
    tx: &mpsc::Sender<AgentEvent>,
    event: AgentEvent,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Some(cancellation) = cancellation {
        tokio::select! {
            biased;
            _ = tx.send(event) => {},
            _ = cancellation.cancelled() => {},
        }
    } else {
        let _ = tx.send(event).await;
    }
}

pub(super) async fn emit_interactive_host_event(
    surface: &crate::fact_control::SessionSurface,
    event: AgentEvent,
) {
    if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
        store.record_event(run_id, event.clone()).await;
    }
    if let Some(sender) = &surface.events {
        send_host_event(sender, event, Some(&surface.cancel)).await;
    }
}

pub(super) async fn emit_gated(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
    streamed_text: bool,
    publication: run_admission::OriginalPublicationKind,
    run_id: &str,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Err(error) = mapped {
        send_host_event(
            tx,
            AgentEvent::Error {
                message: error.to_string(),
            },
            cancellation,
        )
        .await;
        return;
    }
    // Only the original sealed publication can authorize a successful End.
    // Cancellation is an existing Error-frame diagnostic; RunStatus remains
    // the typed status authority. Preserve a real mapped error above rather
    // than replacing it with this clean-cancellation diagnostic.
    let error = match publication {
        run_admission::OriginalPublicationKind::End => {
            emit_settled_to_host(
                tx,
                settled,
                mapped.as_ref().ok(),
                streamed_text,
                cancellation,
            )
            .await;
            return;
        }
        run_admission::OriginalPublicationKind::Cancelled => {
            CodeError::run_cancelled(run_id.to_owned())
        }
        _ => CodeError::Session(format!(
            "Original publication for Run '{run_id}' does not authorize successful End"
        )),
    };
    send_host_event(
        tx,
        AgentEvent::Error {
            message: error.to_string(),
        },
        cancellation,
    )
    .await;
}

#[cfg(test)]
pub(super) async fn emit_settled(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: Option<&AgentResult>,
    streamed_text: bool,
) {
    emit_settled_to_host(tx, settled, mapped, streamed_text, None).await;
}

pub(super) async fn emit_settled_to_host(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: Option<&AgentResult>,
    streamed_text: bool,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    match settled {
        Ok(settled) => {
            let text = settled.view.assistant.clone().unwrap_or_default();
            // The streamed deltas already carried this text to the host; the
            // settled replay only adds the End marker. Re-sending the full
            // text as another delta duplicated every message.
            if !streamed_text {
                send_host_event(
                    tx,
                    AgentEvent::TextDelta { text: text.clone() },
                    cancellation,
                )
                .await;
            }
            // The gate already merged the turn's real verification reports
            // into the mapped result; the End marker must carry them (an
            // empty summary hid Verified/Waived outcomes from every client).
            let verification_summary = match mapped {
                Some(result) => crate::verification::VerificationSummary::from_reports(
                    &result.verification_reports,
                ),
                None => crate::verification::VerificationSummary::from_reports(&[]),
            };
            send_host_event(
                tx,
                AgentEvent::End {
                    text,
                    usage: mapped
                        .map(|result| result.usage.clone())
                        .unwrap_or_default(),
                    verification_summary: Box::new(verification_summary),
                    completion_terminal: mapped
                        .map(|result| result.completion.clone())
                        .unwrap_or_default(),
                    forced_stop: mapped.and_then(|result| result.forced_stop),
                    meta: None,
                },
                cancellation,
            )
            .await;
        }
        Err(error) => {
            send_host_event(
                tx,
                AgentEvent::Error {
                    message: error.to_string(),
                },
                cancellation,
            )
            .await;
        }
    }
}
