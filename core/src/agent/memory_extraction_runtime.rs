use super::execution_state::ExecutionLoopState;
use super::{AgentEvent, AgentLoop};
use crate::llm::Message;
use crate::memory::AgentMemory;
use a3s_memory::{MemoryItem, MemoryType};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MEMORY_EXTRACTION_SYSTEM: &str = "\
You extract durable, reusable memory for a coding agent.
Return JSON only. Do not include markdown.
Keep only facts, preferences, decisions, workflows, and failure lessons that are likely useful in future sessions.
Do not store transient progress, generic praise, raw logs, secrets, credentials, or information that only matters inside the current answer.
Never narrate what the user or assistant did in this turn.
Each memory must be standalone, concise, scoped, and justified.
Write user-facing memory and learning text in plain language without internal orchestration terms.
Return an empty items array when nothing qualifies.";

const MAX_MEMORY_CONTENT_CHARS: usize = 1_200;
const MAX_MEMORY_REASON_CHARS: usize = 320;
const MAX_MEMORY_TAGS: usize = 8;
const MAX_EVOLUTION_PATTERN_CHARS: usize = 96;
const MAX_EVOLUTION_TITLE_CHARS: usize = 64;
const MAX_EVOLUTION_SUMMARY_CHARS: usize = 200;
const MAX_EVOLUTION_INSTRUCTIONS: usize = 4;
const MAX_EVOLUTION_INSTRUCTION_CHARS: usize = 200;
const MAX_RELATED_MEMORY_ITEMS: usize = 5;
const MAX_RELATED_MEMORY_CHARS: usize = 2_000;
const MAX_RELATED_MEMORY_CONTENT_CHARS: usize = 320;
const MAX_DIRECT_TURN_FIELD_CHARS: usize = 2_000;
const MAX_TRANSCRIPT_MESSAGE_CHARS: usize = 1_200;
const SENSITIVE_REDACTION: &str = "[redacted sensitive value]";
const MIN_MEMORY_IMPORTANCE: f32 = 0.70;
const MIN_MEMORY_CONFIDENCE: f32 = 0.75;

#[derive(Debug, Deserialize)]
struct ExtractedMemory {
    #[serde(default, alias = "type")]
    memory_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default, alias = "replaces")]
    supersedes: Vec<String>,
    #[serde(default, alias = "conflictsWith", alias = "conflicts")]
    conflicts_with: Vec<String>,
    #[serde(default)]
    evolution: Option<ExtractedEvolution>,
}

#[derive(Debug, Deserialize)]
struct ExtractedEvolution {
    #[serde(default)]
    kind: String,
    #[serde(default, alias = "patternKey")]
    pattern_key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default, alias = "guidance")]
    instructions: Vec<String>,
}

#[derive(Debug)]
struct EvolutionSignal {
    kind: &'static str,
    pattern_key: String,
    title: String,
    summary: String,
    instructions: Vec<String>,
}

#[derive(Debug, Clone)]
struct RelatedMemoryContext {
    prompt: String,
    allowed_supersedes: HashSet<String>,
}

#[derive(Debug, Clone)]
pub(super) struct MemoryExtractionSnapshot {
    messages: Vec<Message>,
}

impl MemoryExtractionSnapshot {
    pub(super) fn from_state(state: &ExecutionLoopState) -> Self {
        Self {
            messages: state.messages.clone(),
        }
    }
}

struct TurnMemoryExtraction<'a> {
    snapshot: &'a MemoryExtractionSnapshot,
    prompt: &'a str,
    response: &'a str,
    session_id: &'a str,
    event_tx: &'a Option<mpsc::Sender<AgentEvent>>,
    cancel_token: &'a CancellationToken,
}

impl AgentLoop {
    pub(super) async fn schedule_turn_memory_extraction(
        &self,
        state: &ExecutionLoopState,
        prompt: &str,
        response: &str,
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) {
        let snapshot = MemoryExtractionSnapshot::from_state(state);
        let Some(memory) = self.config.memory.as_ref() else {
            return;
        };
        if !memory.llm_extraction_enabled() {
            return;
        }
        if !should_attempt_llm_memory_extraction(&snapshot, prompt, response) {
            return;
        }
        let ticket = memory.enqueue_llm_extraction();

        if event_tx.is_some() {
            let agent = self.clone();
            let prompt = prompt.to_string();
            let response = response.to_string();
            let session_id = session_id.to_string();
            let cancel_token = cancel_token.clone();
            tokio::spawn(async move {
                let no_events = None;
                agent
                    .extract_turn_memories_with_llm(
                        TurnMemoryExtraction {
                            snapshot: &snapshot,
                            prompt: &prompt,
                            response: &response,
                            session_id: &session_id,
                            event_tx: &no_events,
                            cancel_token: &cancel_token,
                        },
                        ticket,
                    )
                    .await;
            });
            return;
        }

        self.extract_turn_memories_with_llm(
            TurnMemoryExtraction {
                snapshot: &snapshot,
                prompt,
                response,
                session_id,
                event_tx,
                cancel_token,
            },
            ticket,
        )
        .await;
    }

    async fn extract_turn_memories_with_llm(
        &self,
        extraction: TurnMemoryExtraction<'_>,
        mut ticket: crate::memory::MemoryExtractionTicket,
    ) {
        let TurnMemoryExtraction {
            snapshot,
            prompt,
            response,
            session_id,
            event_tx,
            cancel_token,
        } = extraction;
        ticket.wait_for_turn().await;
        let Some(memory) = self.config.memory.as_ref().cloned() else {
            return;
        };
        if !memory.llm_extraction_enabled() {
            return;
        }

        let max_items = memory.llm_extraction_max_items().clamp(1, 10);
        let related_memories = related_memories_for_extraction(&memory, prompt, response).await;
        let extraction_prompt = build_extraction_prompt(
            prompt,
            response,
            &turn_transcript(snapshot, memory.llm_extraction_max_input_chars()),
            &related_memories.prompt,
            max_items,
        );
        let messages = [Message::user(&extraction_prompt)];

        let response = match self
            .complete_memory_extraction(&messages, session_id, event_tx, cancel_token)
            .await
        {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!(error = %e, "LLM memory extraction failed");
                return;
            }
        };

        let extracted = match parse_extracted_memories(&response) {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(error = %e, "LLM memory extraction returned invalid JSON");
                return;
            }
        };

        let mut seen = HashSet::new();
        for extracted in extracted.into_iter().take(max_items) {
            let Some((item, supersedes, conflicts_with)) = extracted.into_memory_item(
                &self.tool_context.workspace,
                session_id,
                &related_memories.allowed_supersedes,
            ) else {
                continue;
            };
            let key = normalize_memory_content(&item.content);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            let item = if let Some(existing) =
                similar_existing_memory(&memory, &item, &supersedes, &conflicts_with).await
            {
                merge_duplicate_memory(existing, item)
            } else {
                item
            };

            match memory.remember_item(item).await {
                Ok(item) => {
                    delete_superseded_memories(&memory, &supersedes).await;
                    if let Some(tx) = event_tx {
                        tx.send(AgentEvent::MemoryStored {
                            memory_id: item.id,
                            memory_type: memory_type_label(item.memory_type).to_string(),
                            importance: item.importance,
                            tags: item.tags,
                        })
                        .await
                        .ok();
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to store extracted memory");
                }
            }
        }
    }

    async fn complete_memory_extraction(
        &self,
        messages: &[Message],
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<String> {
        let llm_client = self.scoped_llm_client_for_parts(Some(session_id), event_tx, cancel_token);
        let response = llm_client
            .complete(messages, Some(MEMORY_EXTRACTION_SYSTEM), &[])
            .await?;
        Ok(response.text())
    }
}

impl ExtractedMemory {
    fn into_memory_item(
        self,
        workspace: impl AsRef<Path>,
        session_id: &str,
        allowed_supersedes: &HashSet<String>,
    ) -> Option<(MemoryItem, Vec<String>, Vec<String>)> {
        let workspace = workspace.as_ref();
        let content = compact(self.content.trim(), MAX_MEMORY_CONTENT_CHARS);
        if content.chars().count() < 20 {
            return None;
        }
        if contains_sensitive_memory_material(&content) {
            tracing::warn!(
                "Skipping extracted memory because it appears to contain sensitive material"
            );
            return None;
        }

        let memory_type = parse_durable_memory_type(&self.memory_type)?;
        let source = normalize_extraction_source(self.source)?;
        if !source_matches_memory_type(&source, memory_type) {
            tracing::debug!(
                source,
                memory_type = memory_type_label(memory_type),
                "Skipping extracted memory because its source and type are inconsistent"
            );
            return None;
        }
        let importance = self.importance.filter(|value| value.is_finite())?;
        if !(MIN_MEMORY_IMPORTANCE..=1.0).contains(&importance) {
            return None;
        }
        let confidence = self.confidence.filter(|value| value.is_finite())?;
        if !(MIN_MEMORY_CONFIDENCE..=1.0).contains(&confidence) {
            return None;
        }
        let scope = normalize_memory_scope(self.scope)?;
        let reason = compact(self.reason?.trim(), MAX_MEMORY_REASON_CHARS);
        if reason.chars().count() < 12 || contains_sensitive_memory_material(&reason) {
            return None;
        }
        let supersedes = normalize_supersedes(self.supersedes, allowed_supersedes);
        let conflicts_with = normalize_related_ids(self.conflicts_with, allowed_supersedes);
        let evolution = self.evolution.and_then(|signal| signal.normalize(&source));
        let mut item = MemoryItem::new(content)
            .with_type(memory_type)
            .with_importance(importance)
            .with_tag("llm")
            .with_tag("extracted")
            .with_tag(source.clone())
            .with_metadata("source", source)
            .with_metadata("confidence", format!("{confidence:.2}"))
            .with_metadata("scope", scope)
            .with_metadata("reason", reason)
            .with_metadata("workspace", workspace.display().to_string())
            .with_metadata("session_id", session_id)
            .with_metadata("schema", "a3s.memory.durable.v1");
        if !supersedes.is_empty() {
            item = item
                .with_tag("consolidated")
                .with_metadata("supersedes", supersedes.join(","));
        }
        if !conflicts_with.is_empty() {
            item = item
                .with_tag("conflict")
                .with_metadata("conflicts_with", conflicts_with.join(","));
        }
        if let Some(signal) = evolution {
            item = item
                .with_tag("evolution")
                .with_tag(format!("evolution-{}", signal.kind))
                .with_metadata("evolution_schema", "a3s.evolution.signal.v1")
                .with_metadata("evolution_kind", signal.kind)
                .with_metadata("evolution_pattern", signal.pattern_key)
                .with_metadata("evolution_title", signal.title)
                .with_metadata("evolution_summary", signal.summary)
                .with_metadata(
                    "evolution_instructions",
                    serde_json::to_string(&signal.instructions)
                        .unwrap_or_else(|_| "[]".to_string()),
                );
        }

        for tag in self
            .tags
            .into_iter()
            .filter_map(sanitize_tag)
            .take(MAX_MEMORY_TAGS)
        {
            if !item.tags.contains(&tag) {
                item = item.with_tag(tag);
            }
        }

        Some((item, supersedes, conflicts_with))
    }
}

impl ExtractedEvolution {
    fn normalize(self, source: &str) -> Option<EvolutionSignal> {
        let kind = match self.kind.trim().to_ascii_lowercase().as_str() {
            "preference" if matches!(source, "preference" | "decision") => "preference",
            "skill" if matches!(source, "workflow" | "failure" | "decision") => "skill",
            "okf" | "knowledge" if matches!(source, "project_fact" | "workflow" | "decision") => {
                "okf"
            }
            _ => return None,
        };
        let pattern_key = normalize_evolution_pattern(&self.pattern_key)?;
        let title = self.title.trim();
        let summary = self.summary.trim();
        if title.chars().count() < 4
            || title.chars().count() > MAX_EVOLUTION_TITLE_CHARS
            || summary.chars().count() < 12
            || summary.chars().count() > MAX_EVOLUTION_SUMMARY_CHARS
            || contains_sensitive_memory_material(title)
            || contains_sensitive_memory_material(summary)
        {
            return None;
        }
        let title = title.to_string();
        let summary = summary.to_string();

        let mut seen = HashSet::new();
        let instructions = self
            .instructions
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| {
                let length = value.chars().count();
                (8..=MAX_EVOLUTION_INSTRUCTION_CHARS).contains(&length)
            })
            .filter(|value| !contains_sensitive_memory_material(value))
            .filter(|value| seen.insert(value.to_ascii_lowercase()))
            .take(MAX_EVOLUTION_INSTRUCTIONS)
            .collect::<Vec<_>>();
        if instructions.is_empty() {
            return None;
        }

        Some(EvolutionSignal {
            kind,
            pattern_key,
            title,
            summary,
            instructions,
        })
    }
}

fn normalize_evolution_pattern(raw: &str) -> Option<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for ch in raw.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    let value = segments.join(".");
    if segments.len() < 2
        || value.chars().count() > MAX_EVOLUTION_PATTERN_CHARS
        || segments.iter().any(|segment| segment.len() > 40)
    {
        None
    } else {
        Some(value)
    }
}

fn build_extraction_prompt(
    prompt: &str,
    response: &str,
    transcript: &str,
    related_memories: &str,
    max_items: usize,
) -> String {
    let prompt = redact_sensitive_text(&compact(prompt, MAX_DIRECT_TURN_FIELD_CHARS));
    let response = redact_sensitive_text(&compact(response, MAX_DIRECT_TURN_FIELD_CHARS));
    let transcript = redact_sensitive_text(&compact(transcript, MAX_DIRECT_TURN_FIELD_CHARS));
    format!(
        "\
Extract at most {max_items} durable memories from this completed turn.
Use the related existing memories to avoid duplicates. If the turn refines a related memory, write one updated standalone memory instead of repeating the old wording.
Only put ids from Related existing memories in supersedes when the new memory directly replaces or consolidates them.
Use conflicts_with for directly contradictory related memories that should remain available instead of being deleted.

Return exactly this JSON shape:
{{\"items\":[{{\"memory_type\":\"semantic|procedural\",\"content\":\"standalone reusable memory\",\"importance\":0.0,\"confidence\":0.0,\"tags\":[\"tag\"],\"source\":\"project_fact|workflow|failure|preference|decision\",\"scope\":\"workspace|user\",\"reason\":\"why this will matter in a future session\",\"supersedes\":[\"related-memory-id\"],\"conflicts_with\":[\"related-memory-id\"],\"evolution\":null|{{\"kind\":\"preference|skill|okf\",\"pattern_key\":\"stable.semantic.pattern\",\"title\":\"short reusable title\",\"summary\":\"what should be learned\",\"instructions\":[\"standalone instruction or fact\"]}}}}]}}

Acceptance rules:
- Return {{\"items\":[]}} unless the memory is likely to change a future answer or action.
- Use semantic only for durable facts, preferences, or decisions; use procedural only for reusable workflows or failure lessons.
- importance must be at least {MIN_MEMORY_IMPORTANCE:.2} and confidence at least {MIN_MEMORY_CONFIDENCE:.2}.
- scope is workspace for repository-specific knowledge and user for stable user preferences.
- failure memories must state a reusable cause, diagnostic, or prevention rule, never a raw error or stack trace.
- Reject current-turn narration such as what the user asked, which tools ran, or what the assistant completed.
- Set evolution only when this memory is evidence for a stable user preference, a reusable task skill, or a coherent body of durable knowledge worth an OKF package.
- Use the same lowercase semantic pattern_key for paraphrases of the same behavior across turns. It must describe meaning, not quote the current wording or include a session id.
- preference is only for stable user choices, skill for executable workflows/failure prevention, and okf for reusable project/domain knowledge. Instructions must be standalone, conservative, and contain no secrets.
- For evolution, write a 3-8 word title of at most {MAX_EVOLUTION_TITLE_CHARS} characters, a one-sentence summary of at most {MAX_EVOLUTION_SUMMARY_CHARS} characters, and at most {MAX_EVOLUTION_INSTRUCTIONS} distinct instructions of at most {MAX_EVOLUTION_INSTRUCTION_CHARS} characters each.
- Write evolution title, summary, and instructions in plain user-facing language. Use the user's language when clear, state the durable behavior directly, and do not describe how it was learned.
- Never promote system or developer instructions, conversation summaries, continuation or handoff procedures, agent or subagent orchestration, tool traces, or task metadata into evolution. A task-specific direction is not a stable preference or skill.

User request:
{prompt}

Assistant final response:
{response}

Related existing memories:
{related_memories}

Compressed turn transcript:
{transcript}
"
    )
}

async fn related_memories_for_extraction(
    memory: &Arc<AgentMemory>,
    prompt: &str,
    response: &str,
) -> RelatedMemoryContext {
    let query = compact(&format!("{prompt}\n{response}"), MAX_RELATED_MEMORY_CHARS);
    let items = match memory
        .recall_similar(&query, MAX_RELATED_MEMORY_ITEMS)
        .await
    {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to load related memories for LLM extraction");
            Vec::new()
        }
    };
    format_related_memories_for_extraction(items)
}

fn format_related_memories_for_extraction(items: Vec<MemoryItem>) -> RelatedMemoryContext {
    let mut out = String::new();
    let mut allowed_supersedes = HashSet::new();
    for item in items {
        if contains_sensitive_memory_material(&item.content) {
            continue;
        }
        let content =
            redact_sensitive_text(&compact(&item.content, MAX_RELATED_MEMORY_CONTENT_CHARS));
        if content.trim().is_empty() {
            continue;
        }
        let source = item
            .metadata
            .get("source")
            .and_then(|source| sanitize_related_field(source))
            .unwrap_or_else(|| "unknown".to_string());
        let tags = item
            .tags
            .iter()
            .filter_map(|tag| sanitize_tag(tag.clone()))
            .take(MAX_MEMORY_TAGS)
            .collect::<Vec<_>>();
        let mut line = serde_json::Map::new();
        line.insert("id".to_string(), serde_json::json!(item.id));
        line.insert(
            "type".to_string(),
            serde_json::json!(memory_type_label(item.memory_type)),
        );
        line.insert("importance".to_string(), serde_json::json!(item.importance));
        line.insert("source".to_string(), serde_json::json!(source));
        line.insert("tags".to_string(), serde_json::json!(tags));
        let supersedes = metadata_relation_ids(&item, "supersedes");
        if !supersedes.is_empty() {
            line.insert("supersedes".to_string(), serde_json::json!(supersedes));
        }
        let conflicts_with = metadata_relation_ids(&item, "conflicts_with");
        if !conflicts_with.is_empty() {
            line.insert(
                "conflicts_with".to_string(),
                serde_json::json!(conflicts_with),
            );
        }
        line.insert("content".to_string(), serde_json::json!(content));
        allowed_supersedes.insert(item.id.clone());
        out.push_str(&serde_json::Value::Object(line).to_string());
        out.push('\n');
        if out.chars().count() >= MAX_RELATED_MEMORY_CHARS {
            return RelatedMemoryContext {
                prompt: compact(&out, MAX_RELATED_MEMORY_CHARS),
                allowed_supersedes,
            };
        }
    }

    let prompt = if out.trim().is_empty() {
        "None".to_string()
    } else {
        out
    };
    RelatedMemoryContext {
        prompt,
        allowed_supersedes,
    }
}

fn metadata_relation_ids(item: &MemoryItem, key: &str) -> Vec<String> {
    item.metadata
        .get(key)
        .map(|value| {
            value
                .split(',')
                .filter_map(sanitize_related_memory_id)
                .take(MAX_RELATED_MEMORY_ITEMS)
                .collect()
        })
        .unwrap_or_default()
}

fn sanitize_related_memory_id(raw: &str) -> Option<String> {
    let id = raw.trim();
    if id.is_empty() || id.chars().count() > 120 {
        return None;
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '/'))
    {
        return None;
    }
    Some(id.to_string())
}

fn sanitize_related_field(value: &str) -> Option<String> {
    let value = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .take(40)
        .collect::<String>();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn normalize_supersedes(
    supersedes: Vec<String>,
    allowed_supersedes: &HashSet<String>,
) -> Vec<String> {
    normalize_related_ids(supersedes, allowed_supersedes)
}

fn normalize_related_ids(ids: Vec<String>, allowed_supersedes: &HashSet<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| allowed_supersedes.contains(id))
        .filter(|id| seen.insert(id.clone()))
        .take(MAX_RELATED_MEMORY_ITEMS)
        .collect()
}

fn turn_transcript(snapshot: &MemoryExtractionSnapshot, max_chars: usize) -> String {
    let mut lines = Vec::new();
    let mut remaining = max_chars;
    for message in snapshot.messages.iter().rev() {
        if remaining == 0 {
            break;
        }
        let text = compact(&message.text(), MAX_TRANSCRIPT_MESSAGE_CHARS);
        if text.trim().is_empty() {
            continue;
        }
        let line = format!("{}: {}", message.role, text);
        let line = compact(&line, remaining);
        remaining = remaining.saturating_sub(line.chars().count());
        lines.push(line);
    }
    lines.reverse();
    lines.join("\n\n")
}

fn parse_extracted_memories(text: &str) -> anyhow::Result<Vec<ExtractedMemory>> {
    let json = extract_json(text).ok_or_else(|| anyhow::anyhow!("missing JSON object or array"))?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    let values = if let Some(items) = value.get("items") {
        items
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("items must be an array"))?
    } else {
        value
            .as_array()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("expected JSON object with items or an array"))?
    };

    Ok(values
        .into_iter()
        .filter_map(
            |value| match serde_json::from_value::<ExtractedMemory>(value) {
                Ok(item) => Some(item),
                Err(e) => {
                    tracing::warn!(error = %e, "Skipping malformed extracted memory item");
                    None
                }
            },
        )
        .collect())
}

fn extract_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Some(trimmed.to_string());
    }

    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim().to_string());
        }
    }

    let obj_start = trimmed.find('{');
    let obj_end = trimmed.rfind('}');
    match (obj_start, obj_end) {
        (Some(start), Some(end)) if start < end => Some(trimmed[start..=end].to_string()),
        _ => None,
    }
}

fn parse_durable_memory_type(value: &str) -> Option<MemoryType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "semantic" => Some(MemoryType::Semantic),
        "procedural" => Some(MemoryType::Procedural),
        _ => None,
    }
}

fn normalize_extraction_source(source: Option<String>) -> Option<String> {
    let source = source?;
    match source.trim().to_ascii_lowercase().as_str() {
        "project_fact" | "workflow" | "failure" | "preference" | "decision" => Some(
            source
                .trim()
                .to_ascii_lowercase()
                .chars()
                .take(40)
                .collect(),
        ),
        _ => None,
    }
}

fn normalize_memory_scope(scope: Option<String>) -> Option<String> {
    match scope?.trim().to_ascii_lowercase().as_str() {
        "workspace" | "project" | "repository" | "repo" => Some("workspace".to_string()),
        "user" | "personal" => Some("user".to_string()),
        _ => None,
    }
}

fn source_matches_memory_type(source: &str, memory_type: MemoryType) -> bool {
    match source {
        "project_fact" | "preference" => memory_type == MemoryType::Semantic,
        "workflow" | "failure" => memory_type == MemoryType::Procedural,
        "decision" => matches!(memory_type, MemoryType::Semantic | MemoryType::Procedural),
        _ => false,
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Episodic => "episodic",
        MemoryType::Semantic => "semantic",
        MemoryType::Procedural => "procedural",
        MemoryType::Working => "working",
    }
}

fn sanitize_tag(tag: String) -> Option<String> {
    let tag = tag
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .take(40)
        .collect::<String>();
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

fn normalize_memory_content(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

async fn delete_superseded_memories(memory: &Arc<AgentMemory>, supersedes: &[String]) {
    for id in supersedes {
        if let Err(e) = memory.forget(id).await {
            tracing::warn!(memory_id = %id, error = %e, "Failed to delete superseded memory");
        }
    }
}

fn merge_duplicate_memory(mut existing: MemoryItem, mut extracted: MemoryItem) -> MemoryItem {
    let duplicate_id = extracted.id.clone();
    extracted.id = existing.id;
    extracted.timestamp = existing.timestamp;
    extracted.importance = existing.importance.max(extracted.importance);
    extracted.access_count = existing.access_count;
    extracted.last_accessed = existing.last_accessed;

    for tag in existing.tags.drain(..) {
        if !extracted.tags.contains(&tag) {
            extracted.tags.push(tag);
        }
    }

    for (key, value) in existing.metadata.drain() {
        extracted.metadata.entry(key).or_insert(value);
    }
    record_duplicate_memory_metadata(&mut extracted.metadata, &duplicate_id);

    extracted.content_lower = extracted.content.to_lowercase();
    extracted
}

fn record_duplicate_memory_metadata(
    metadata: &mut std::collections::HashMap<String, String>,
    duplicate_id: &str,
) {
    let count = metadata
        .get("duplicate_count")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    metadata.insert("duplicate_count".to_string(), count.to_string());
    metadata.insert(
        "last_duplicate_at".to_string(),
        chrono::Utc::now().to_rfc3339(),
    );
    if !duplicate_id.trim().is_empty() {
        metadata.insert("last_observation_id".to_string(), duplicate_id.to_string());
        metadata
            .entry("duplicate_ids".to_string())
            .and_modify(|existing| {
                if !existing
                    .split(',')
                    .map(str::trim)
                    .any(|seen| seen == duplicate_id)
                {
                    if !existing.trim().is_empty() {
                        existing.push(',');
                    }
                    existing.push_str(duplicate_id);
                }
            })
            .or_insert_with(|| duplicate_id.to_string());
    }
}

async fn similar_existing_memory(
    memory: &Arc<AgentMemory>,
    item: &MemoryItem,
    supersedes: &[String],
    conflicts_with: &[String],
) -> Option<MemoryItem> {
    let candidates = match memory.recall_similar(&item.content, 5).await {
        Ok(candidates) => candidates,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to check extracted memory for duplicates");
            return None;
        }
    };

    candidates
        .into_iter()
        .filter(|candidate| {
            !supersedes.contains(&candidate.id) && !conflicts_with.contains(&candidate.id)
        })
        .find(|candidate| memory_contents_are_duplicates(&candidate.content, &item.content))
}

fn memory_contents_are_duplicates(existing: &str, extracted: &str) -> bool {
    let existing = normalize_memory_content(existing);
    let extracted = normalize_memory_content(extracted);
    !existing.is_empty() && existing == extracted
}

fn compact(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push_str("\n... (truncated)");
    out
}

fn contains_sensitive_memory_material(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key-----") {
        return true;
    }
    sensitive_memory_patterns()
        .iter()
        .any(|pattern| pattern.is_match(text))
}

fn redact_sensitive_text(text: &str) -> String {
    if text.to_ascii_lowercase().contains("-----begin ") {
        return SENSITIVE_REDACTION.to_string();
    }
    let mut redacted = text.to_string();
    for pattern in sensitive_memory_patterns() {
        redacted = pattern
            .replace_all(&redacted, SENSITIVE_REDACTION)
            .into_owned();
    }
    redacted
}

fn sensitive_memory_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            vec![
                Regex::new(r"(?i)\bsk-[a-z0-9_-]{16,}\b").expect("valid OpenAI key regex"),
                Regex::new(r"(?i)\b(?:ghp|gho|ghu|ghs|ghr)_[a-z0-9_]{16,}\b")
                    .expect("valid GitHub token regex"),
                Regex::new(r"(?i)\bgithub_pat_[a-z0-9_]{20,}\b")
                    .expect("valid GitHub PAT regex"),
                Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("valid AWS access key regex"),
                Regex::new(
                    r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|pwd|access[_-]?key|private[_-]?key)\s*(?:=|:|is)\s*['"]?[^\s'",;]{8,}"#,
                )
                .expect("valid secret assignment regex"),
            ]
        })
        .as_slice()
}

fn should_attempt_llm_memory_extraction(
    _snapshot: &MemoryExtractionSnapshot,
    prompt: &str,
    response: &str,
) -> bool {
    !prompt.trim().is_empty() && !response.trim().is_empty()
}

#[cfg(test)]
#[path = "memory_extraction_runtime/tests.rs"]
mod tests;
