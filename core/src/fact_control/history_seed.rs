//! Seeding and reading back conversation history from facts.

use super::*;

#[derive(Clone)]
pub(super) struct RecordedCall {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) args: serde_json::Value,
    pub(super) text: String,
    pub(super) reasoning: Option<String>,
}

pub(super) struct SeededTool {
    pub(super) id: String,
    pub(super) text: String,
    pub(super) is_error: bool,
    pub(super) images: Vec<crate::llm::Attachment>,
}

pub(super) fn seeded_tool(message: &Message) -> Option<SeededTool> {
    for block in &message.content {
        let crate::llm::ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } = block
        else {
            continue;
        };
        let is_error = is_error.unwrap_or(false);
        let (text, images) = match content {
            crate::llm::ToolResultContentField::Text(text) => (text.clone(), Vec::new()),
            crate::llm::ToolResultContentField::Blocks(blocks) => {
                let mut text = String::new();
                let mut images = Vec::new();
                for block in blocks {
                    match block {
                        crate::llm::ToolResultContent::Text { text: part } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(part);
                        }
                        crate::llm::ToolResultContent::Image { source } => {
                            let Ok(bytes) = base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                source.data.as_bytes(),
                            ) else {
                                continue;
                            };
                            if bytes.is_empty() {
                                continue;
                            }
                            images.push(crate::llm::Attachment::new(
                                bytes,
                                source.media_type.clone(),
                            ));
                        }
                    }
                }
                (text, images)
            }
        };
        if images.is_empty() && !is_error {
            continue;
        }
        return Some(SeededTool {
            id: tool_use_id.clone(),
            text,
            is_error,
            images,
        });
    }
    None
}

/// Seed the original transcript without admitting or running an infer.
pub(crate) fn seed_history(log: &FileLog, thread: &str, history: &[Message]) -> Result<()> {
    let existing = log.read(thread).map_err(|error| anyhow::anyhow!(error))?;
    if existing.iter().any(|fact| {
        fact.kind != crate::standing_context::STANDING_CONTEXT_FACT_KIND
            && fact.kind != crate::work_policy::POLICY_KIND
    }) {
        return Ok(());
    }
    let mut turn = 0u64;
    let mut cycle = 0u64;
    for message in history {
        match message.role.as_str() {
            "user" => {
                turn += 1;
                cycle = 0;
                if let Some(seeded) = seeded_tool(message) {
                    // The tool turn consumes `infer:{turn}:0`. The new
                    // prompt is the next user message and gets its own infer.
                    log.append(
                        thread,
                        &[message_fact(
                            format!("m-hist-{turn}"),
                            seeded_user_text(message),
                        )],
                        None,
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                    append_seeded_tool(log, thread, turn, &seeded)?;
                } else {
                    log.append(
                        thread,
                        &[message_fact(
                            format!("m-hist-{turn}"),
                            seeded_user_text(message),
                        )],
                        None,
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                }
            }
            "assistant" => {
                let payload = serde_json::to_value(ModelDecision::Text {
                    text: message.text(),
                })
                .map_err(|error| anyhow::anyhow!(error))?;
                log.append(
                    thread,
                    &[NewFact {
                        kind: "model.turn".into(),
                        key: format!("model:{turn}:{cycle}"),
                        payload,
                    }],
                    Some(&format!("infer:{turn}:{cycle}")),
                )
                .map_err(|error| anyhow::anyhow!(error))?;
                cycle += 1;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn append_seeded_tool(
    log: &FileLog,
    thread: &str,
    turn: u64,
    seeded: &SeededTool,
) -> Result<()> {
    let output = match fact_tool_runner_value(&seeded.text, seeded.is_error, &seeded.images) {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    let decision = ModelDecision::Tool {
        call: ToolCall {
            id: seeded.id.clone(),
            name: "tool".into(),
            args: serde_json::json!({}),
            needs_confirmation: false,
            text: None,
            reasoning: None,
        },
        also: Vec::new(),
    };
    let payload = serde_json::to_value(decision).map_err(|error| anyhow::anyhow!(error))?;
    log.append(
        thread,
        &[NewFact {
            kind: "model.turn".into(),
            key: format!("model:{turn}:0"),
            payload,
        }],
        Some(&format!("infer:{turn}:0")),
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    log.append(
        thread,
        &[NewFact {
            kind: "tool.result".into(),
            key: format!("tool-result:{turn}:0:{}", seeded.id),
            payload: serde_json::json!({
                "toolCallId": seeded.id,
                "ok": !seeded.is_error,
                "output": output,
            }),
        }],
        None,
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

pub(super) fn seeded_user_text(message: &Message) -> String {
    let tool_text = message
        .content
        .iter()
        .filter_map(|block| {
            if let crate::llm::ContentBlock::ToolResult { content, .. } = block {
                Some(content.as_text())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if !tool_text.is_empty() {
        return tool_text;
    }
    message.text()
}

pub(super) fn assistant_tool_call(call: &RecordedCall) -> Message {
    let mut content = Vec::new();
    if !call.text.is_empty() {
        content.push(crate::llm::ContentBlock::Text {
            text: call.text.clone(),
        });
    }
    content.push(crate::llm::ContentBlock::ToolUse {
        id: call.id.clone(),
        name: call.name.clone(),
        input: call.args.clone(),
    });
    Message {
        role: "assistant".into(),
        content,
        reasoning_content: call.reasoning.clone(),
        transcript_text: None,
        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
    }
}

pub(super) fn user_message_fact(key: String, text: &str, attachments: &[Attachment]) -> NewFact {
    if attachments.is_empty() {
        return message_fact(key, text);
    }
    NewFact {
        kind: "user.message".into(),
        key,
        payload: serde_json::json!({
            "text": text,
            "images": attachments
                .iter()
                .map(|image| serde_json::json!({
                    "media_type": image.media_type,
                    "data": base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &image.data,
                    ),
                }))
                .collect::<Vec<_>>(),
        }),
    }
}

pub(super) fn attachments_from_user_payload(payload: &serde_json::Value) -> Vec<Attachment> {
    let Some(images) = payload.get("images").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    images
        .iter()
        .filter_map(|image| {
            let media_type = image.get("media_type").and_then(|value| value.as_str())?;
            if !media_type.starts_with("image/") {
                return None;
            }
            let data = image.get("data").and_then(|value| value.as_str())?;
            let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .ok()
                .filter(|bytes| !bytes.is_empty())?;
            Some(Attachment::new(bytes, media_type))
        })
        .collect()
}

/// Image bytes for user lines that the effect fold still has after the latest
/// compaction. `kept` lines are already in the scheduler transcript and are
/// not paired with these facts.
pub(super) fn user_attachment_plan(
    workspace: &Path,
    thread: &str,
) -> (usize, std::collections::VecDeque<Vec<Attachment>>) {
    let Ok(facts) = read_workspace_facts(workspace, thread) else {
        return (0, std::collections::VecDeque::new());
    };
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "compaction.done")
        .map(|index| index + 1)
        .unwrap_or(0);
    let kept_len = if start > 0 {
        facts[start - 1]
            .payload
            .get("kept")
            .and_then(|value| value.as_array())
            .map(|kept| kept.len())
            .unwrap_or(0)
    } else {
        0
    };
    let mut images = std::collections::VecDeque::new();
    for fact in facts.iter().skip(start) {
        match fact.kind.as_str() {
            "user.message" => images.push_back(attachments_from_user_payload(&fact.payload)),
            "question.answered" => images.push_back(Vec::new()),
            _ => {}
        }
    }
    (kept_len, images)
}

pub(super) fn images_for_user_line(
    index: usize,
    summary_offset: usize,
    kept_len: usize,
    queue: &mut std::collections::VecDeque<Vec<Attachment>>,
) -> Vec<Attachment> {
    if index < summary_offset {
        return Vec::new();
    }
    let request_index = index - summary_offset;
    if request_index < kept_len {
        return Vec::new();
    }
    queue.pop_front().unwrap_or_default()
}

/// Indexes of `assistant\ntool <name>\n<json>` lines that belong to a closed
/// scheduler batch: one or more of those records, then the same number of
/// `tool` or `tool-error` results. The writer in the effect fold is the
/// source of that shape. An unmatched record stays in the transcript.
pub(super) fn scheduler_tool_record_indexes(lines: &[String]) -> std::collections::HashSet<usize> {
    const RECORD_PREFIX: &str = "assistant\ntool ";
    let mut indexes = std::collections::HashSet::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        if !lines[cursor].starts_with(RECORD_PREFIX) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < lines.len() && lines[cursor].starts_with(RECORD_PREFIX) {
            cursor += 1;
        }
        let records = cursor - start;
        let mut results = 0;
        while cursor + results < lines.len()
            && (lines[cursor + results].starts_with("tool\n")
                || lines[cursor + results].starts_with("tool-error\n"))
        {
            results += 1;
        }
        if results == records {
            for index in start..cursor {
                indexes.insert(index);
            }
            cursor += results;
        } else {
            cursor = start + 1;
        }
    }
    indexes
}

pub(super) fn folded_message_with_images(
    role: &str,
    body: &str,
    images: Vec<Attachment>,
) -> Message {
    if role == "assistant" {
        Message::assistant(body)
    } else {
        user_message_with_images(body, images)
    }
}

pub(super) fn user_message_with_images(body: &str, images: Vec<Attachment>) -> Message {
    if images.is_empty() {
        Message::user(body)
    } else {
        Message::user_with_attachments(body, &images)
    }
}

/// Reattach original call metadata to completed results in the folded view.
/// `split_compact_tail` keeps an ordered suffix, including tool results. Keep
/// the corresponding suffix of original calls and verify its result bytes;
/// never fabricate an identity when the original facts cannot be read/paired.
pub(super) fn recorded_tool_calls(
    workspace: &Path,
    thread: &str,
) -> Result<Vec<RecordedCall>, ActorError> {
    let facts = read_workspace_facts(workspace, thread).map_err(|error| ActorError::Handler {
        key: "model-input-tool-history".into(),
        message: format!("cannot read original tool-call facts: {error}"),
    })?;
    let mut pending = std::collections::VecDeque::new();
    let mut completed: Vec<(RecordedCall, String)> = Vec::new();
    for fact in facts {
        if fact.kind == "model.turn"
            && fact.payload.get("kind").and_then(|kind| kind.as_str()) == Some("tool")
        {
            let ModelDecision::Tool { call, also } =
                serde_json::from_value::<ModelDecision>(fact.payload).map_err(|error| {
                    ActorError::Schema(format!("invalid original tool-call metadata: {error}"))
                })?
            else {
                return Err(ActorError::Schema("invalid original tool decision".into()));
            };
            pending.clear();
            let mut ids = std::collections::HashSet::new();
            for call in std::iter::once(call).chain(also) {
                if !ids.insert(call.id.clone()) {
                    return Err(ActorError::Schema(
                        "duplicate original tool-call identity in batch".into(),
                    ));
                }
                pending.push_back(RecordedCall {
                    id: call.id,
                    name: call.name,
                    args: call.args,
                    text: call.text.unwrap_or_default(),
                    reasoning: call.reasoning.filter(|text| !text.is_empty()),
                });
            }
        } else if fact.kind == "tool.result" {
            let receipt = fact
                .payload
                .get("receipt")
                .map(|receipt| {
                    serde_json::from_value::<a3s_effect::ToolExecutionResultV1>(receipt.clone())
                        .map_err(|error| {
                            ActorError::Schema(format!("invalid original tool receipt: {error}"))
                        })
                })
                .transpose()?;
            if let Some(receipt) = &receipt {
                receipt.validate()?;
            }
            #[derive(serde::Deserialize)]
            struct ResultPayload {
                #[serde(default, rename = "toolCallId")]
                tool_call_id: Option<String>,
                ok: bool,
                output: String,
            }
            let payload: ResultPayload = serde_json::from_value(fact.payload).map_err(|error| {
                ActorError::Schema(format!("invalid original tool-result metadata: {error}"))
            })?;
            let call = pending.pop_front().ok_or_else(|| {
                ActorError::Schema("tool result has no original pending call".into())
            })?;
            // Legacy results may omit the id. Explicit ids must agree with
            // the scheduler's serial batch order, just as the Moore fold checks.
            if payload.tool_call_id.is_some_and(|id| id != call.id)
                || receipt.is_some_and(|receipt| receipt.identity.tool_call_id != call.id)
            {
                return Err(ActorError::Schema(
                    "tool-result identity disagrees with the original pending call".into(),
                ));
            }
            let role = if payload.ok { "tool" } else { "tool-error" };
            completed.push((call, format!("{role}\n{}", payload.output)));
        } else if fact.kind == "compaction.done" {
            #[derive(serde::Deserialize)]
            struct CompactionPayload {
                #[serde(default)]
                kept: Vec<String>,
            }
            let payload: CompactionPayload =
                serde_json::from_value(fact.payload).map_err(|error| {
                    ActorError::Schema(format!("invalid retained tool history: {error}"))
                })?;
            let retained: Vec<_> = payload
                .kept
                .iter()
                .filter(|line| line.starts_with("tool\n") || line.starts_with("tool-error\n"))
                .collect();
            let start = completed.len().checked_sub(retained.len()).ok_or_else(|| {
                ActorError::Schema("retained tool results have no original calls".into())
            })?;
            if completed[start..]
                .iter()
                .zip(retained)
                .any(|((_, original), kept)| original != kept)
            {
                return Err(ActorError::Schema(
                    "retained tool results disagree with the original suffix".into(),
                ));
            }
            completed.drain(..start);
            pending.clear();
        }
    }
    Ok(completed.into_iter().map(|(call, _)| call).collect())
}

pub(super) fn recorded_tool_messages(
    recorded: &mut std::vec::IntoIter<RecordedCall>,
    body: &str,
    is_error: bool,
) -> Result<[Message; 2], ActorError> {
    let call = recorded.next().ok_or_else(|| {
        ActorError::Schema("folded tool result has no original call metadata".into())
    })?;
    Ok([
        assistant_tool_call(&call),
        message_from_folded_tool(&call.id, body, is_error),
    ])
}

pub(super) fn recorded_call_from(call: &serde_json::Value) -> RecordedCall {
    let id = call
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let name = call
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let args = call
        .get("args")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let text = call
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let reasoning = call
        .get("reasoning")
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    RecordedCall {
        id: id.to_string(),
        name: name.to_string(),
        args,
        text,
        reasoning,
    }
}

pub(super) fn workspace_boundary_error(message: &str) -> bool {
    message.contains("escapes workspace") || message.contains("Workspace boundary")
}

pub(super) fn repeat_note_prefix(name: &str) -> String {
    format!("\n\n[a3s host: `{name}` returned byte-identical output for identical arguments")
}

/// Mechanical feedback when a call repeats the name, arguments and output of the
/// previous call with the same name and arguments in this run. It never blocks.
/// A JSON result stays parseable for its consumer, so it carries no note.
pub(super) fn repeated_result_note(
    repeats: &Mutex<std::collections::HashMap<String, (String, u32)>>,
    name: &str,
    args: &serde_json::Value,
    output: &str,
) -> Option<String> {
    let call = crate::content_digest::digest_bytes(
        "a3s.code.tool-repeat.call.v1",
        format!("{name}\n{args}").as_bytes(),
    );
    let result =
        crate::content_digest::digest_bytes("a3s.code.tool-repeat.result.v1", output.as_bytes());
    let mut repeats = repeats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = repeats.entry(call).or_default();
    if entry.0 == result {
        entry.1 += 1;
    } else {
        *entry = (result, 1);
    }
    let structured = serde_json::from_str::<serde_json::Value>(output).is_ok();
    (entry.1 > 1 && !structured).then(|| {
        format!(
            "{} {} times in a row this run; it observed nothing new.]",
            repeat_note_prefix(name),
            entry.1
        )
    })
}

pub(super) fn visible_tool_output(
    agent: Option<&crate::agent::AgentLoop>,
    output: String,
) -> String {
    match agent {
        Some(agent) => agent.sanitize_tool_output(&output),
        None => output,
    }
}

pub(super) async fn emit_tool_request_bound(tools: &ExecutorTools, call: &a3s_effect::ToolCall) {
    let Ok(snapshot) = crate::harness_evidence::ToolRequestSnapshotV1::capture(
        &call.id,
        &call.name,
        &call.args,
        crate::harness_evidence::ToolRequestOriginV1::Agent,
    ) else {
        return;
    };
    tools
        .emit(crate::agent::AgentEvent::ToolRequestBound {
            tool_id: call.id.clone(),
            tool_name: call.name.clone(),
            snapshot,
        })
        .await;
}
