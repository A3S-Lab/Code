//! Portable caches are exported only from the original committed tool log.

use super::*;

pub(super) async fn save_committed(
    surface: &SessionSurface,
    checkpoint: &CheckpointState,
    attempted_seq: &AtomicU64,
) -> Result<(), ActorError> {
    let facts = surface_fact_log(surface)?.read(&thread_for_session(&surface.session_id))?;
    replay_unsettled_mutations_from_facts(surface, &facts);
    let debt = receipts::fold_debt(&facts);
    let expected_binding = checkpoint
        .capability_binding
        .as_ref()
        .map(|binding| {
            crate::content_digest::digest_json("a3s.code.tool-admission-binding.v1", binding)
                .map_err(|error| ActorError::Schema(error.to_string()))
        })
        .transpose()?;
    let thread = thread_for_session(&surface.session_id);
    // A recovery may read receipts from its source Run. Export only when
    // this exact admitted Run has added a terminal; the original checkpoint
    // and log remain the authority for its older work.
    let latest = facts.iter().rev().find(|fact| {
        if fact.kind != "tool.result" {
            return false;
        }
        let Some(value) = fact.payload.get("receipt") else {
            return false;
        };
        let Ok(result) = serde_json::from_value::<a3s_effect::ToolExecutionResultV1>(value.clone())
        else {
            return false;
        };
        result.identity.admission.as_ref().is_some_and(|admission| {
            admission.run_id == checkpoint.run_id
                && admission.thread_id == thread
                && expected_binding.as_deref() == Some(admission.capability_binding_digest.as_str())
        }) && debt
            .results
            .iter()
            .any(|(_, committed)| committed == &result)
    });
    let Some(latest) = latest else {
        return Ok(());
    };
    if latest.seq <= attempted_seq.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Ledger and conversation come from this same committed prefix. A
    // concurrent background append belongs to a later projection.
    let messages = checkpoint_messages_from_facts(&facts);
    let mutations = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let usage = surface
        .usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let calls = debt
        .results
        .iter()
        .filter(|(_, result)| {
            result.identity.admission.as_ref().is_some_and(|admission| {
                admission.run_id == checkpoint.run_id
                    && admission.thread_id == thread
                    && expected_binding.as_deref()
                        == Some(admission.capability_binding_digest.as_str())
            })
        })
        .count();
    let durable_source =
        crate::store::SessionFactSourceV1::from_facts(&checkpoint.session_id, &facts)
            .map_err(|error| ActorError::Schema(error.to_string()))?;
    if durable_source != checkpoint.durable_source {
        return Err(ActorError::Schema(
            "checkpoint durable source changed during its Run".into(),
        ));
    }
    let turn = checkpoint.turns.fetch_add(1, Ordering::SeqCst) + 1;
    let checkpoint_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    checkpoint
        .sink
        .save_checkpoint(&crate::loop_checkpoint::LoopCheckpoint {
            schema_version: crate::loop_checkpoint::LoopCheckpoint::schema_for(
                durable_source.as_ref(),
            ),
            run_id: checkpoint.run_id.clone(),
            session_id: checkpoint.session_id.clone(),
            capability_binding: checkpoint.capability_binding.clone(),
            standing_context_digest: checkpoint.standing_context_digest.clone(),
            durable_source,
            turn,
            messages,
            total_usage: usage,
            tool_calls_count: calls,
            verification_reports: reports,
            convergence: crate::loop_checkpoint::LoopConvergenceState {
                mutations,
                ..Default::default()
            },
            checkpoint_ms,
        })
        .await;
    // The existing sink is best effort and has no durable-success return.
    // This cursor records a completed export attempt, never a durable
    // checkpoint or a settlement marker. Recovery still uses the original log.
    attempted_seq.store(latest.seq, Ordering::SeqCst);
    Ok(())
}

/// Export the provider wire shape, with identities and attachments from the
/// same original prefix as the ledger. The compaction display transcript is
/// intentionally text-only and cannot substitute for a tool checkpoint.
pub(super) fn checkpoint_messages_from_facts(facts: &[a3s_effect::Fact]) -> Vec<Message> {
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "compaction.done")
        .map_or(0, |index| index + 1);
    let mut messages = compaction_transcript_from_facts(&facts[..start]);
    for (index, fact) in facts.iter().enumerate().skip(start) {
        let message = match fact.kind.as_str() {
            "user.message" => fact.payload.get("text").and_then(|text| {
                text.as_str().map(|text| {
                    user_message_with_images(text, attachments_from_user_payload(&fact.payload))
                })
            }),
            "model.turn" => {
                let Ok(ModelDecision::Tool { call, also }) =
                    serde_json::from_value::<ModelDecision>(fact.payload.clone())
                else {
                    if let Some(message) = message_from_fact(fact) {
                        messages.push(message);
                    }
                    continue;
                };
                // A batch may contain refused or still-pending calls. A
                // checkpoint must not invent their results or leave dangling
                // wire ToolUse blocks. Their original facts and receipt debt
                // remain untouched; this cache includes committed calls only.
                let terminal_ids = facts[index + 1..]
                    .iter()
                    .take_while(|next| {
                        !matches!(
                            next.kind.as_str(),
                            "model.turn" | "compaction.done" | "user.message"
                        )
                    })
                    .filter(|next| next.kind == "tool.result")
                    .filter_map(|next| next.payload.get("toolCallId")?.as_str())
                    .collect::<std::collections::HashSet<_>>();
                let mut content = Vec::new();
                if let Some(text) = call.text.clone().filter(|text| !text.is_empty()) {
                    content.push(crate::llm::ContentBlock::Text { text });
                }
                let reasoning_content = call.reasoning.clone();
                content.extend(std::iter::once(call).chain(also).filter_map(|call| {
                    terminal_ids.contains(call.id.as_str()).then_some(
                        crate::llm::ContentBlock::ToolUse {
                            id: call.id,
                            name: call.name,
                            input: call.args,
                        },
                    )
                }));
                (!content.is_empty()).then_some(Message {
                    role: "assistant".into(),
                    content,
                    reasoning_content,
                    transcript_text: None,
                    transcript_visibility: crate::llm::TranscriptVisibility::Wire,
                })
            }
            "tool.result" => fact
                .payload
                .get("toolCallId")
                .and_then(serde_json::Value::as_str)
                .zip(
                    fact.payload
                        .get("output")
                        .and_then(serde_json::Value::as_str),
                )
                .map(|(id, output)| {
                    message_from_folded_tool(
                        id,
                        output,
                        fact.payload.get("ok").and_then(serde_json::Value::as_bool) == Some(false),
                    )
                }),
            _ => message_from_fact(fact),
        };
        if let Some(message) = message {
            messages.push(message);
        }
    }
    messages
}

#[cfg(test)]
mod tests;
