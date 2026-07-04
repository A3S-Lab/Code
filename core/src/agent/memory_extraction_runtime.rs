use super::execution_state::ExecutionLoopState;
use super::{AgentEvent, AgentLoop};
use crate::budget::BudgetDecision;
use crate::llm::Message;
use crate::memory::AgentMemory;
use a3s_memory::{MemoryItem, MemoryType};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

const MEMORY_EXTRACTION_SYSTEM: &str = "\
You extract durable, reusable memory for a coding agent.
Return JSON only. Do not include markdown.
Keep only facts, preferences, decisions, workflows, and failure lessons that are likely useful in future sessions.
Do not store transient progress, generic praise, raw logs, secrets, credentials, or information that only matters inside the current answer.
Each memory must be standalone and concise.";

const MAX_MEMORY_CONTENT_CHARS: usize = 1_200;
const MAX_MEMORY_TAGS: usize = 8;
const MAX_RELATED_MEMORY_ITEMS: usize = 5;
const MAX_RELATED_MEMORY_CHARS: usize = 2_000;
const MAX_RELATED_MEMORY_CONTENT_CHARS: usize = 320;
const MAX_DIRECT_TURN_FIELD_CHARS: usize = 2_000;
const MAX_TRANSCRIPT_MESSAGE_CHARS: usize = 1_200;
const SENSITIVE_REDACTION: &str = "[redacted sensitive value]";

#[derive(Debug, Deserialize)]
struct ExtractedMemory {
    #[serde(default, alias = "type")]
    memory_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, alias = "replaces")]
    supersedes: Vec<String>,
    #[serde(default, alias = "conflictsWith", alias = "conflicts")]
    conflicts_with: Vec<String>,
}

#[derive(Debug, Clone)]
struct RelatedMemoryContext {
    prompt: String,
    allowed_supersedes: HashSet<String>,
}

#[derive(Debug, Clone)]
pub(super) struct MemoryExtractionSnapshot {
    messages: Vec<Message>,
    tool_calls_count: usize,
    recent_tool_signatures: Vec<String>,
}

impl MemoryExtractionSnapshot {
    pub(super) fn from_state(state: &ExecutionLoopState) -> Self {
        Self {
            messages: state.messages.clone(),
            tool_calls_count: state.tool_calls_count,
            recent_tool_signatures: state.recent_tool_signatures(),
        }
    }
}

impl AgentLoop {
    pub(super) async fn schedule_turn_memory_extraction(
        &self,
        state: &ExecutionLoopState,
        prompt: &str,
        response: &str,
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
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

        if event_tx.is_some() {
            let agent = self.clone();
            let prompt = prompt.to_string();
            let response = response.to_string();
            let session_id = session_id.to_string();
            tokio::spawn(async move {
                let no_events = None;
                agent
                    .extract_turn_memories_with_llm(
                        &snapshot,
                        &prompt,
                        &response,
                        &session_id,
                        &no_events,
                    )
                    .await;
            });
            return;
        }

        self.extract_turn_memories_with_llm(&snapshot, prompt, response, session_id, event_tx)
            .await;
    }

    pub(super) async fn extract_turn_memories_with_llm(
        &self,
        snapshot: &MemoryExtractionSnapshot,
        prompt: &str,
        response: &str,
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        let Some(memory) = self.config.memory.as_ref().cloned() else {
            return;
        };
        if !memory.llm_extraction_enabled() {
            return;
        }
        if !should_attempt_llm_memory_extraction(snapshot, prompt, response) {
            return;
        }
        let Some(_permit) = memory.try_begin_llm_extraction() else {
            tracing::debug!(
                "Skipping LLM memory extraction because another extraction is already running"
            );
            return;
        };

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
            .complete_memory_extraction_with_budget(&messages, session_id, event_tx)
            .await
        {
            Ok(Some(response)) => response,
            Ok(None) => return,
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
                prompt,
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
                existing.merge_duplicate(item)
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

    async fn complete_memory_extraction_with_budget(
        &self,
        messages: &[Message],
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(guard) = &self.config.budget_guard {
            let estimate = estimate_prompt_tokens(messages, Some(MEMORY_EXTRACTION_SYSTEM));
            match guard.check_before_llm(session_id, estimate).await {
                BudgetDecision::Allow => {}
                BudgetDecision::SoftLimit {
                    resource,
                    consumed,
                    limit,
                    message,
                } => {
                    emit_budget_threshold(event_tx, resource, "soft", consumed, limit, message)
                        .await;
                }
                BudgetDecision::Deny { resource, reason } => {
                    emit_budget_threshold(
                        event_tx,
                        resource.clone(),
                        "hard",
                        0.0,
                        0.0,
                        Some(reason.clone()),
                    )
                    .await;
                    tracing::warn!(
                        resource = %resource,
                        reason = %reason,
                        "Skipping LLM memory extraction because budget guard denied the call"
                    );
                    return Ok(None);
                }
            }
        }

        let response = self
            .llm_client
            .complete(messages, Some(MEMORY_EXTRACTION_SYSTEM), &[])
            .await?;
        if let Some(guard) = &self.config.budget_guard {
            guard.record_after_llm(session_id, &response.usage).await;
        }
        Ok(Some(response.text()))
    }
}

impl ExtractedMemory {
    fn into_memory_item(
        self,
        prompt: &str,
        session_id: &str,
        allowed_supersedes: &HashSet<String>,
    ) -> Option<(MemoryItem, Vec<String>, Vec<String>)> {
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

        let memory_type = parse_memory_type(&self.memory_type);
        let source = normalize_extraction_source(self.source);
        let prompt_metadata = redact_sensitive_text(&compact(prompt, 500));
        let supersedes = normalize_supersedes(self.supersedes, allowed_supersedes);
        let conflicts_with = normalize_related_ids(self.conflicts_with, allowed_supersedes);
        let mut item = MemoryItem::new(content)
            .with_type(memory_type)
            .with_importance(self.importance.unwrap_or(0.65).clamp(0.1, 1.0))
            .with_tag("llm")
            .with_tag("extracted")
            .with_metadata("source", source)
            .with_metadata("session_id", session_id)
            .with_metadata("prompt", prompt_metadata);
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
{{\"items\":[{{\"memory_type\":\"semantic|procedural|episodic\",\"content\":\"standalone memory\",\"importance\":0.0,\"tags\":[\"tag\"],\"source\":\"project_fact|workflow|failure|preference|decision\",\"supersedes\":[\"related-memory-id\"],\"conflicts_with\":[\"related-memory-id\"]}}]}}

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

fn parse_memory_type(value: &str) -> MemoryType {
    match value.trim().to_ascii_lowercase().as_str() {
        "semantic" => MemoryType::Semantic,
        "procedural" => MemoryType::Procedural,
        "working" => MemoryType::Working,
        _ => MemoryType::Episodic,
    }
}

fn normalize_extraction_source(source: Option<String>) -> String {
    let Some(source) = source else {
        return "llm_extractor".to_string();
    };
    match source.trim().to_ascii_lowercase().as_str() {
        "project_fact" | "workflow" | "failure" | "preference" | "decision" => source
            .trim()
            .to_ascii_lowercase()
            .chars()
            .take(40)
            .collect(),
        _ => "llm_extractor".to_string(),
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
    if existing.is_empty() || extracted.is_empty() {
        return false;
    }
    if existing == extracted {
        return true;
    }

    let existing_terms = durable_memory_terms(&existing);
    let extracted_terms = durable_memory_terms(&extracted);
    if existing_terms.len() < 4 || extracted_terms.len() < 4 {
        return false;
    }

    let overlap = existing_terms.intersection(&extracted_terms).count();
    let union = existing_terms.len() + extracted_terms.len() - overlap;
    union > 0 && overlap as f32 / union as f32 >= 0.85
}

fn durable_memory_terms(content: &str) -> HashSet<String> {
    content
        .split(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/')))
        .map(str::trim)
        .filter(|term| term.chars().count() >= 3)
        .filter(|term| !is_memory_stopword(term))
        .map(ToOwned::to_owned)
        .collect()
}

fn is_memory_stopword(term: &str) -> bool {
    matches!(
        term,
        "the"
            | "and"
            | "for"
            | "with"
            | "after"
            | "before"
            | "from"
            | "that"
            | "this"
            | "when"
            | "then"
            | "than"
            | "into"
            | "must"
            | "should"
            | "would"
            | "could"
            | "about"
            | "memory"
    )
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

async fn emit_budget_threshold(
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    resource: String,
    kind: &str,
    consumed: f64,
    limit: f64,
    message: Option<String>,
) {
    if let Some(tx) = event_tx {
        tx.send(AgentEvent::BudgetThresholdHit {
            resource,
            kind: kind.to_string(),
            consumed,
            limit,
            message,
        })
        .await
        .ok();
    }
}

fn estimate_prompt_tokens(messages: &[Message], system: Option<&str>) -> usize {
    let mut chars = system.map(|s| s.len()).unwrap_or(0);
    for message in messages {
        chars += message.text().len();
    }
    chars / 4
}

fn should_attempt_llm_memory_extraction(
    snapshot: &MemoryExtractionSnapshot,
    prompt: &str,
    response: &str,
) -> bool {
    if prompt.trim().is_empty() || response.trim().is_empty() {
        return false;
    }

    if tool_activity_warrants_extraction(snapshot) {
        return true;
    }

    let combined = format!("{prompt}\n{response}").to_ascii_lowercase();
    if combined.chars().count() >= 600 {
        return true;
    }

    durable_signal_keywords()
        .iter()
        .any(|needle| combined.contains(needle))
}

fn durable_signal_keywords() -> &'static [&'static str] {
    &[
        "remember",
        "always",
        "never",
        "prefer",
        "preference",
        "decision",
        "decided",
        "root cause",
        "bug",
        "fix",
        "fixed",
        "failed",
        "failure",
        "regression",
        "workflow",
        "convention",
        "rule",
        "project",
        "repo",
        "module",
        "crate",
        "api",
        "config",
        "migration",
        "请记住",
        "记住",
        "记忆",
        "偏好",
        "决策",
        "决定",
        "约定",
        "规范",
        "工作流",
        "流程",
        "失败",
        "错误",
        "修复",
        "根因",
        "项目",
        "仓库",
        "配置",
        "迁移",
        "模块",
        "接口",
    ]
}

fn tool_activity_warrants_extraction(snapshot: &MemoryExtractionSnapshot) -> bool {
    if snapshot.tool_calls_count == 0 {
        return false;
    }

    let signatures = &snapshot.recent_tool_signatures;
    if signatures.is_empty() {
        return true;
    }

    signatures.iter().any(|signature| {
        tool_signature_is_error(signature)
            || !tool_signature_name(signature).is_some_and(is_read_only_memory_tool)
    })
}

fn tool_signature_name(signature: &str) -> Option<&str> {
    signature.split_once(':').map(|(name, _)| name)
}

fn tool_signature_is_error(signature: &str) -> bool {
    signature
        .rsplit_once("=>")
        .map(|(_, status)| status.trim() == "error")
        .unwrap_or(true)
}

fn is_read_only_memory_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "read" | "grep" | "glob" | "ls" | "web_fetch" | "web_search"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(state: &ExecutionLoopState) -> MemoryExtractionSnapshot {
        MemoryExtractionSnapshot::from_state(state)
    }

    #[test]
    fn parses_fenced_extraction_json() {
        let items = parse_extracted_memories(
            r#"```json
{"items":[{"memory_type":"semantic","content":"A3S memory should store durable project facts.","importance":0.8,"tags":["A3S","Memory"],"source":"project_fact"}]}
```"#,
        )
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].memory_type, "semantic");
    }

    #[test]
    fn parser_keeps_valid_items_when_sibling_is_malformed() {
        let items = parse_extracted_memories(
            r#"{"items":[
                {"memory_type":"semantic","content":"A3S memory extraction runs after significant completed turns.","importance":0.8,"tags":["memory"],"source":"project_fact"},
                {"memory_type":"semantic","content":42,"importance":"high"},
                {"memory_type":"procedural","content":"Run memory extraction tests after changing extraction parsing behavior.","importance":0.7,"tags":["tests"],"source":"workflow"}
            ]}"#,
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        assert!(items[0].content.contains("completed turns"));
        assert!(items[1].content.contains("extraction parsing"));
    }

    #[test]
    fn missing_extracted_content_is_skipped_during_item_conversion() {
        let extracted = ExtractedMemory {
            memory_type: "semantic".to_string(),
            content: String::new(),
            importance: Some(0.8),
            tags: vec!["memory".to_string()],
            source: Some("project_fact".to_string()),
            supersedes: vec![],
            conflicts_with: vec![],
        };

        assert!(extracted
            .into_memory_item("remember memory behavior", "sess-1", &HashSet::new())
            .is_none());
    }

    #[test]
    fn extracted_memory_becomes_tagged_item() {
        let extracted = ExtractedMemory {
            memory_type: "procedural".to_string(),
            content: "Run focused memory tests after changing FileMemoryStore.".to_string(),
            importance: Some(0.9),
            tags: vec!["Memory!".to_string(), "Tests".to_string()],
            source: Some("workflow".to_string()),
            supersedes: vec![],
            conflicts_with: vec![],
        };

        let (item, supersedes, conflicts_with) = extracted
            .into_memory_item("optimize memory", "sess-1", &HashSet::new())
            .unwrap();
        assert!(supersedes.is_empty());
        assert!(conflicts_with.is_empty());
        assert_eq!(item.memory_type, MemoryType::Procedural);
        assert!(item.tags.contains(&"llm".to_string()));
        assert!(item.tags.contains(&"memory".to_string()));
        assert_eq!(item.metadata.get("source").unwrap(), "workflow");
    }

    #[test]
    fn extracted_memory_skips_sensitive_content() {
        let extracted = ExtractedMemory {
            memory_type: "semantic".to_string(),
            content: "The production API key is sk-1234567890abcdef1234567890abcdef.".to_string(),
            importance: Some(0.9),
            tags: vec!["secret".to_string()],
            source: Some("project_fact".to_string()),
            supersedes: vec![],
            conflicts_with: vec![],
        };

        assert!(extracted
            .into_memory_item("remember the key", "sess-1", &HashSet::new())
            .is_none());
    }

    #[test]
    fn extracted_memory_redacts_sensitive_prompt_metadata() {
        let extracted = ExtractedMemory {
            memory_type: "procedural".to_string(),
            content: "Use environment variables when configuring provider credentials.".to_string(),
            importance: Some(0.8),
            tags: vec!["config".to_string()],
            source: Some("workflow".to_string()),
            supersedes: vec![],
            conflicts_with: vec![],
        };

        let (item, _, _) = extracted
            .into_memory_item(
                "provider api_key = sk-1234567890abcdef1234567890abcdef",
                "sess-1",
                &HashSet::new(),
            )
            .unwrap();

        let prompt = item.metadata.get("prompt").unwrap();
        assert!(prompt.contains(SENSITIVE_REDACTION));
        assert!(!prompt.contains("sk-1234567890abcdef"));
    }

    #[test]
    fn extraction_prompt_redacts_sensitive_turn_fields() {
        let prompt = build_extraction_prompt(
            "provider api_key = sk-1234567890abcdef1234567890abcdef",
            "Use token: ghp_1234567890abcdef1234567890abcdef",
            "assistant: password = supersecret123",
            "None",
            3,
        );

        assert!(prompt.contains(SENSITIVE_REDACTION));
        assert!(!prompt.contains("sk-1234567890abcdef"));
        assert!(!prompt.contains("ghp_1234567890abcdef"));
        assert!(!prompt.contains("supersecret123"));
    }

    #[test]
    fn extraction_source_is_whitelisted() {
        let extracted = ExtractedMemory {
            memory_type: "semantic".to_string(),
            content: "A3S memory uses gated LLM extraction after completed turns.".to_string(),
            importance: Some(0.8),
            tags: vec![],
            source: Some("api_key = sk-1234567890abcdef1234567890abcdef".to_string()),
            supersedes: vec![],
            conflicts_with: vec![],
        };

        let (item, _, _) = extracted
            .into_memory_item("memory design", "sess-1", &HashSet::new())
            .unwrap();
        assert_eq!(item.metadata.get("source").unwrap(), "llm_extractor");
    }

    #[test]
    fn extracted_memory_records_allowed_supersedes() {
        let allowed_id = uuid::Uuid::new_v4().to_string();
        let ignored_id = uuid::Uuid::new_v4().to_string();
        let allowed = HashSet::from([allowed_id.clone()]);
        let extracted = ExtractedMemory {
            memory_type: "procedural".to_string(),
            content: "Run focused memory and file-store tests after changing memory persistence."
                .to_string(),
            importance: Some(0.9),
            tags: vec!["memory".to_string()],
            source: Some("workflow".to_string()),
            supersedes: vec![allowed_id.clone(), ignored_id],
            conflicts_with: vec![],
        };

        let (item, supersedes, conflicts_with) = extracted
            .into_memory_item("memory design", "sess-1", &allowed)
            .unwrap();

        assert_eq!(supersedes, vec![allowed_id.clone()]);
        assert!(conflicts_with.is_empty());
        assert!(item.tags.contains(&"consolidated".to_string()));
        assert_eq!(item.metadata.get("supersedes").unwrap(), &allowed_id);
    }

    #[test]
    fn extracted_memory_records_allowed_conflicts() {
        let conflict_id = uuid::Uuid::new_v4().to_string();
        let ignored_id = uuid::Uuid::new_v4().to_string();
        let allowed = HashSet::from([conflict_id.clone()]);
        let extracted = ExtractedMemory {
            memory_type: "semantic".to_string(),
            content: "This project currently prefers workspace-local memory stores.".to_string(),
            importance: Some(0.75),
            tags: vec!["memory".to_string()],
            source: Some("decision".to_string()),
            supersedes: vec![],
            conflicts_with: vec![conflict_id.clone(), ignored_id],
        };

        let (item, supersedes, conflicts_with) = extracted
            .into_memory_item("memory design", "sess-1", &allowed)
            .unwrap();

        assert!(supersedes.is_empty());
        assert_eq!(conflicts_with, vec![conflict_id.clone()]);
        assert!(item.tags.contains(&"conflict".to_string()));
        assert_eq!(item.metadata.get("conflicts_with").unwrap(), &conflict_id);
    }

    #[test]
    fn related_memories_are_formatted_as_json_lines() {
        let item = MemoryItem::new("Run focused memory store tests after FileMemoryStore changes.")
            .with_type(MemoryType::Procedural)
            .with_importance(0.84)
            .with_tag("Memory!")
            .with_metadata("source", "workflow");

        let formatted = format_related_memories_for_extraction(vec![item.clone()]);

        assert!(formatted.prompt.contains(&format!(r#""id":"{}""#, item.id)));
        assert!(formatted.prompt.contains(r#""type":"procedural""#));
        assert!(formatted.prompt.contains(r#""source":"workflow""#));
        assert!(formatted.prompt.contains(r#""tags":["memory"]"#));
        assert!(formatted.prompt.contains("FileMemoryStore changes"));
        assert!(formatted.allowed_supersedes.contains(&item.id));
    }

    #[test]
    fn related_memories_include_existing_relation_metadata() {
        let item = MemoryItem::new(
            "Use the consolidated memory workflow for project-specific preferences.",
        )
        .with_type(MemoryType::Semantic)
        .with_metadata("supersedes", "old-preference, bad id with spaces")
        .with_metadata("conflicts_with", "legacy-default,<script>");

        let formatted = format_related_memories_for_extraction(vec![item]);

        assert!(formatted
            .prompt
            .contains(r#""supersedes":["old-preference"]"#));
        assert!(formatted
            .prompt
            .contains(r#""conflicts_with":["legacy-default"]"#));
        assert!(!formatted.prompt.contains("bad id with spaces"));
        assert!(!formatted.prompt.contains("<script>"));
    }

    #[test]
    fn related_memories_skip_sensitive_items() {
        let secret = MemoryItem::new("The provider token is sk-1234567890abcdef1234567890abcdef.")
            .with_type(MemoryType::Semantic);
        let safe = MemoryItem::new("Prefer environment variables for provider credentials.")
            .with_type(MemoryType::Procedural);

        let formatted = format_related_memories_for_extraction(vec![secret, safe]);

        assert!(!formatted.prompt.contains("sk-1234567890abcdef"));
        assert!(formatted.prompt.contains("environment variables"));
        assert_eq!(formatted.allowed_supersedes.len(), 1);
    }

    #[tokio::test]
    async fn related_memories_are_loaded_for_extraction_prompt() {
        let memory = Arc::new(AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new())));
        memory
            .remember(
                MemoryItem::new(
                    "Run focused memory store tests after changing FileMemoryStore behavior.",
                )
                .with_type(MemoryType::Procedural)
                .with_tag("memory"),
            )
            .await
            .unwrap();

        let related = related_memories_for_extraction(
            &memory,
            "remember FileMemoryStore testing workflow",
            "Use focused memory tests.",
        )
        .await;
        let prompt = build_extraction_prompt("p", "r", "t", &related.prompt, 2);

        assert!(prompt.contains("Related existing memories"));
        assert!(prompt.contains("FileMemoryStore behavior"));
        assert!(prompt.contains("avoid duplicates"));
        assert_eq!(related.allowed_supersedes.len(), 1);
    }

    #[test]
    fn duplicate_memory_detection_is_conservative() {
        assert!(memory_contents_are_duplicates(
            "Run focused memory store tests after changing FileMemoryStore behavior.",
            "  run focused memory store tests after changing FileMemoryStore behavior.  "
        ));
        assert!(memory_contents_are_duplicates(
            "Run focused memory store regression tests after changing FileMemoryStore behavior.",
            "Run focused memory store tests after changing FileMemoryStore behavior."
        ));
        assert!(!memory_contents_are_duplicates(
            "Run focused memory store tests after changing FileMemoryStore behavior.",
            "Prefer HCL configuration files for repository-level product settings."
        ));
    }

    #[test]
    fn extraction_gate_skips_trivial_turns() {
        let state = ExecutionLoopState::new(&[]);
        assert!(!should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "hi",
            "hello"
        ));
    }

    #[test]
    fn extraction_gate_accepts_tool_turns() {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "run tests",
            "tests passed"
        ));
    }

    #[test]
    fn extraction_gate_skips_short_successful_read_only_tool_turns() {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        state.remember_tool_signature("read", &serde_json::json!({"path":"README.md"}), false);

        assert!(!should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "read README",
            "Here is the README."
        ));
    }

    #[test]
    fn extraction_gate_accepts_read_only_tool_turns_with_memory_language() {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        state.remember_tool_signature("grep", &serde_json::json!({"pattern":"memory"}), false);

        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "remember that this repo keeps memory config in HCL",
            "Got it."
        ));
    }

    #[test]
    fn extraction_gate_accepts_tool_failures() {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        state.remember_tool_signature("read", &serde_json::json!({"path":"missing"}), true);

        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "read missing file",
            "The read failed."
        ));
    }

    #[test]
    fn extraction_gate_accepts_write_capable_tool_turns() {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        state.remember_tool_signature("bash", &serde_json::json!({"cmd":"cargo test"}), false);

        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "run tests",
            "tests passed"
        ));
    }

    #[test]
    fn extraction_gate_accepts_explicit_memory_language() {
        let state = ExecutionLoopState::new(&[]);
        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "remember that this repo prefers HCL config",
            "Got it."
        ));
    }

    #[test]
    fn extraction_gate_accepts_chinese_memory_language() {
        let state = ExecutionLoopState::new(&[]);
        assert!(should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "请记住：这个项目的记忆系统默认必须启用 LLM 抽取",
            "已记录这个设计约定。"
        ));
    }
}
