//! Read-side projections of a workspace fact log.

use super::*;

pub fn read_workspace_facts(workspace: &Path, thread: &str) -> Result<Vec<a3s_effect::Fact>> {
    let log = FileLog::open(crate::session_folder::effect_log_dir(workspace, thread))
        .map_err(|error| anyhow::anyhow!(error))?;
    log.read(thread).map_err(|error| anyhow::anyhow!(error))
}

/// One tool result as the model received it, for the session context view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultWeight {
    pub(crate) tool: String,
    pub(crate) tool_call_id: String,
    pub(crate) bytes: usize,
    /// The same tool, arguments and text already appeared earlier in the thread.
    pub(crate) repeat: bool,
}

/// Every tool result in `thread`, paired with its call in scheduler order. A
/// read-only view: a result without a recorded call keeps an empty tool name.
pub(crate) fn tool_result_weights(workspace: &Path, thread: &str) -> Result<Vec<ToolResultWeight>> {
    let mut pending = std::collections::VecDeque::new();
    let mut seen = std::collections::HashSet::new();
    let mut weights = Vec::new();
    for fact in read_workspace_facts(workspace, thread)? {
        if fact.kind == "model.turn" {
            if let Ok(ModelDecision::Tool { call, also }) =
                serde_json::from_value::<ModelDecision>(fact.payload)
            {
                pending = std::iter::once(call).chain(also).collect();
            }
        } else if fact.kind == "tool.result" {
            let output = fact
                .payload
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let text = decode_fact_tool_result(output)
                .map_or_else(|| output.to_string(), |envelope| envelope.text);
            let (tool, id, args) = pending
                .pop_front()
                .map(|call| (call.name, call.id, call.args))
                .unwrap_or_default();
            let observed = text
                .rsplit_once(&repeat_note_prefix(&tool))
                .map_or(text.as_str(), |(observed, _)| observed);
            let key = crate::content_digest::digest_bytes(
                "a3s.code.context-view.result.v1",
                format!("{tool}\n{args}\n{observed}").as_bytes(),
            );
            weights.push(ToolResultWeight {
                repeat: !seen.insert(key),
                tool,
                tool_call_id: id,
                bytes: text.len(),
            });
        }
    }
    Ok(weights)
}

/// Messages the next model call can still see, plus assistant text that the
/// live fold does not replay. A prior `compaction.done` summary is the prefix.
pub(crate) fn compaction_transcript(workspace: &Path, session_id: &str) -> Result<Vec<Message>> {
    let facts = read_workspace_facts(workspace, &thread_for_session(session_id))?;
    Ok(compaction_transcript_from_facts(&facts))
}

/// Project the latest transcript from the original pinned log, also on Err.
pub(crate) fn retained_transcript(surface: &SessionSurface) -> Result<Vec<Message>, ActorError> {
    let log = surface_fact_log(surface)?;
    let facts = log.read(&thread_for_session(&surface.session_id))?;
    let messages = checkpoints::checkpoint_messages_from_facts(&facts);
    *surface
        .transcript
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = messages.clone();
    Ok(messages)
}

pub(crate) fn compaction_transcript_from_facts(facts: &[a3s_effect::Fact]) -> Vec<Message> {
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "compaction.done")
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut messages = Vec::new();
    if start > 0 {
        let done = &facts[start - 1];
        if let Some(summary) = done
            .payload
            .get("summary")
            .and_then(|value| value.as_str())
            .filter(|summary| !summary.is_empty())
        {
            messages.push(Message::user(&format!(
                "{}{summary}",
                crate::prompts::CONTEXT_SUMMARY_PREFIX
            )));
        }
        if let Some(kept) = done.payload.get("kept").and_then(|value| value.as_array()) {
            messages.extend(kept.iter().filter_map(message_from_kept_line));
        }
    }
    for fact in facts.iter().skip(start) {
        if let Some(message) = message_from_fact(fact) {
            messages.push(message);
        }
    }
    messages
}

/// Record a host summary without resuming the actor.
///
/// The next `user.message` folds this fact first, so the model sees the
/// summary instead of the older transcript. Leaving the log idle here does
/// not start an inference.
pub(crate) fn append_compaction_done(
    log: &FileLog,
    writer: &a3s_effect::FileLogWriterFence,
    session_id: &str,
    summary: &str,
    source: Option<&crate::store::SessionFactSourceV1>,
) -> Result<()> {
    let thread = thread_for_session(session_id);
    if !writer.is_for(log)? {
        anyhow::bail!("compaction requires the original log writer");
    }
    let existing = log.read(&thread).map_err(|error| anyhow::anyhow!(error))?;
    if crate::store::SessionFactSourceV1::from_facts(session_id, &existing)?.as_ref() != source {
        anyhow::bail!("compaction durable source changed before append");
    }
    let fact = NewFact {
        kind: "compaction.done".into(),
        key: format!("compaction:{}", existing.len()),
        payload: serde_json::json!({ "summary": summary }),
    };
    log.append(&thread, &[fact], None)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

pub(super) fn message_from_kept_line(line: &serde_json::Value) -> Option<Message> {
    let line = line.as_str()?;
    if let Some(output) = line.strip_prefix("tool-error\n") {
        return Some(Message::user(&format!("Tool error: {output}")));
    }
    if let Some(output) = line.strip_prefix("tool\n") {
        return Some(Message::user(&format!("Tool result: {output}")));
    }
    // Effect auto-compact stores the recent tail as folded lines. A later
    // `/compact` only reseals what this transcript still contains.
    if let Some(body) = line.strip_prefix("user\n") {
        return Some(Message::user(body));
    }
    line.strip_prefix("assistant\n").map(Message::assistant)
}

pub(super) fn message_from_fact(fact: &a3s_effect::Fact) -> Option<Message> {
    match fact.kind.as_str() {
        "user.message" | "question.answered" => {
            let text = fact.payload.get("text")?.as_str()?;
            Some(Message::user(text))
        }
        "model.turn" => {
            let decided = serde_json::from_value::<ModelDecision>(fact.payload.clone()).ok()?;
            match decided {
                ModelDecision::Text { text } => Some(Message::assistant(&text)),
                ModelDecision::Tool { call, also } => {
                    let rendered = std::iter::once(call)
                        .chain(also)
                        .map(|call| format!("Tool call {} ({}): {}", call.name, call.id, call.args))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(Message::assistant(&rendered))
                }
                ModelDecision::Question { question, .. } => Some(Message::assistant(&question)),
            }
        }
        "tool.result" => {
            let output = fact.payload.get("output")?.as_str()?;
            Some(Message::user(&format!("Tool result: {output}")))
        }
        _ => None,
    }
}

pub(super) fn exit_to_error(error: Exit<ActorError>) -> anyhow::Error {
    let message = match error {
        Exit::Die(message) => message,
        Exit::Interrupt => "cancelled".to_string(),
        Exit::Fail(ActorError::Handler { message, .. }) => message,
        Exit::Fail(ActorError::Defect(message)) => message,
        Exit::Fail(error @ ActorError::StepLimit { .. }) => format!("StepLimit: {error}"),
        Exit::Fail(other) => other.to_string(),
    };
    anyhow::anyhow!(message)
}

pub fn phase_name(phase: CodingPhase) -> &'static str {
    match phase {
        CodingPhase::Idle => "idle",
        CodingPhase::Infer => "infer",
        CodingPhase::Compact => "compact",
        CodingPhase::Confirm => "confirm",
        CodingPhase::Question => "question",
        CodingPhase::Tool => "tool",
        CodingPhase::Deny => "deny",
        CodingPhase::Done => "done",
    }
}

/// Map a permission decision onto the tool call the fold will store.
pub fn project_needs_confirmation(policy: &PermissionPolicy, tool_name: &str) -> bool {
    confirmation_required(policy, tool_name)
}
