use crate::hooks::PreContextPerceptionEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct IntentDetectionResult {
    pub(super) detected_intent: String,
    pub(super) confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) target_hints: Option<TargetHints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TargetHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) target_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) target_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) domain: Option<String>,
}

pub(super) fn detect_local_context_perception_intent(
    prompt: &str,
    session_id: &str,
    workspace: &str,
) -> Option<PreContextPerceptionEvent> {
    let lower = prompt.to_lowercase();

    let intents: &[(&[&str], &str)] = &[
        (
            &[
                "where is",
                "where are",
                "find the file",
                "find all",
                "find files",
                "who wrote",
                "locate",
                "search for",
                "look for",
                "search",
            ],
            "locate",
        ),
        (
            &[
                "how does",
                "what does",
                "explain",
                "understand",
                "what is this",
                "how does this work",
            ],
            "understand",
        ),
        (
            &[
                "remember",
                "earlier",
                "before",
                "previously",
                "last time",
                "past",
                "previous",
            ],
            "retrieve",
        ),
        (
            &[
                "how is organized",
                "project structure",
                "what files",
                "show me the structure",
                "explore",
            ],
            "explore",
        ),
        (
            &[
                "why did",
                "why is",
                "cause",
                "reason",
                "what happened",
                "why does",
            ],
            "reason",
        ),
        (
            &["is this correct", "verify", "validate", "check if", "debug"],
            "validate",
        ),
        (
            &[
                "difference between",
                "compare",
                "versus",
                " vs ",
                "different from",
            ],
            "compare",
        ),
        (
            &[
                "status",
                "progress",
                "how far",
                "history",
                "what's the current",
            ],
            "track",
        ),
    ];

    let target_type = if lower.contains("function") || lower.contains("method") {
        "function"
    } else if lower.contains("file") || lower.contains("config") {
        "file"
    } else if lower.contains("class") {
        "entity"
    } else if lower.contains("module") || lower.contains("package") {
        "module"
    } else if lower.contains("test") {
        "test"
    } else {
        "unknown"
    };

    let matched_intent = intents
        .iter()
        .find(|(patterns, _)| patterns.iter().any(|p| lower.contains(p)));

    matched_intent.map(|(patterns, intent)| PreContextPerceptionEvent {
        session_id: session_id.to_string(),
        intent: intent.to_string(),
        target_type: target_type.to_string(),
        target_name: extract_target_name_from_prompt(prompt, patterns),
        domain: detect_domain_from_prompt(prompt),
        query: Some(prompt.to_string()),
        working_directory: workspace.to_string(),
        urgency: "normal".to_string(),
    })
}

pub(super) fn build_pre_context_perception_from_intent(
    result: IntentDetectionResult,
    prompt: &str,
    session_id: &str,
    workspace: &str,
) -> PreContextPerceptionEvent {
    let target_hints = result.target_hints;
    PreContextPerceptionEvent {
        session_id: session_id.to_string(),
        intent: result.detected_intent,
        target_type: target_hints
            .as_ref()
            .and_then(|h| h.target_type.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        target_name: target_hints
            .as_ref()
            .and_then(|h| h.target_name.clone())
            .unwrap_or_else(|| extract_target_name_from_prompt(prompt, &[])),
        domain: target_hints
            .as_ref()
            .and_then(|h| h.domain.clone())
            .unwrap_or_else(|| detect_domain_from_prompt(prompt)),
        query: Some(prompt.to_string()),
        working_directory: workspace.to_string(),
        urgency: "normal".to_string(),
    }
}

pub(super) fn detect_language_hint(prompt: &str) -> Option<String> {
    if prompt
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        return Some("zh".to_string());
    }
    if prompt
        .chars()
        .any(|c| ('\u{3040}'..='\u{309f}').contains(&c) || ('\u{30a0}'..='\u{30ff}').contains(&c))
    {
        return Some("ja".to_string());
    }
    if prompt
        .chars()
        .any(|c| ('\u{ac00}'..='\u{d7af}').contains(&c))
    {
        return Some("ko".to_string());
    }
    if prompt
        .chars()
        .any(|c| ('\u{0600}'..='\u{06ff}').contains(&c))
    {
        return Some("ar".to_string());
    }
    if prompt
        .chars()
        .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
    {
        return Some("ru".to_string());
    }
    None
}

fn extract_target_name_from_prompt(prompt: &str, _patterns: &[&str]) -> String {
    if let Some(start) = prompt.find('"') {
        if let Some(end) = prompt[start + 1..].find('"') {
            return prompt[start + 1..start + 1 + end].to_string();
        }
    }

    if let Some(start) = prompt.find('\'') {
        if let Some(end) = prompt[start + 1..].find('\'') {
            return prompt[start + 1..start + 1 + end].to_string();
        }
    }

    if let Some(start) = prompt.find('`') {
        if let Some(end) = prompt[start + 1..].find('`') {
            return prompt[start + 1..start + 1 + end].to_string();
        }
    }

    let words: Vec<&str> = prompt.split_whitespace().collect();
    if words.len() > 2 {
        for word in words.iter() {
            if word.len() > 3
                && !["where", "what", "find", "the", "how", "is", "are"].contains(word)
            {
                return word.to_string();
            }
        }
    }

    String::new()
}

fn detect_domain_from_prompt(prompt: &str) -> String {
    let lower = prompt.to_lowercase();

    if lower.contains("rust") || lower.contains("cargo") || lower.contains(".rs") {
        "rust".to_string()
    } else if lower.contains("javascript")
        || lower.contains("typescript")
        || lower.contains("node")
        || lower.contains(".js")
        || lower.contains(".ts")
    {
        "javascript".to_string()
    } else if lower.contains("python") || lower.contains(".py") {
        "python".to_string()
    } else if lower.contains("go") || lower.contains(".go") {
        "go".to_string()
    } else if lower.contains("java") || lower.contains(".java") {
        "java".to_string()
    } else if lower.contains("docker") || lower.contains("container") {
        "docker".to_string()
    } else if lower.contains("kubernetes") || lower.contains("k8s") {
        "kubernetes".to_string()
    } else if lower.contains("sql")
        || lower.contains("database")
        || lower.contains("postgres")
        || lower.contains("mysql")
    {
        "database".to_string()
    } else if lower.contains("api") || lower.contains("rest") || lower.contains("grpc") {
        "api".to_string()
    } else if lower.contains("auth")
        || lower.contains("login")
        || lower.contains("password")
        || lower.contains("token")
    {
        "security".to_string()
    } else if lower.contains("test") || lower.contains("spec") || lower.contains("mock") {
        "testing".to_string()
    } else {
        "general".to_string()
    }
}

#[cfg(feature = "ahp")]
fn estimate_tokens(text: &str) -> usize {
    (text.len() / 4).max(1)
}

#[cfg(feature = "ahp")]
fn ahp_context_result(
    items: Vec<crate::context::ContextItem>,
) -> Option<crate::context::ContextResult> {
    if items.is_empty() {
        return None;
    }

    let total_tokens = items.iter().map(|item| item.token_count).sum();
    Some(crate::context::ContextResult {
        items,
        total_tokens,
        provider: "ahp_harness".to_string(),
        truncated: false,
    })
}

#[cfg(feature = "ahp")]
pub(super) fn injected_context_to_results(
    injected: crate::ahp::InjectedContext,
) -> Vec<crate::context::ContextResult> {
    use crate::context::{ContextItem, ContextType};

    let mut results = Vec::new();

    let fact_items = injected
        .facts
        .into_iter()
        .map(|fact| {
            let token_count = estimate_tokens(&fact.content);
            ContextItem::new(
                uuid::Uuid::new_v4().to_string(),
                ContextType::Resource,
                fact.content,
            )
            .with_source(fact.source)
            .with_provenance("ahp_fact")
            .with_priority(0.75)
            .with_trust(fact.confidence)
            .with_freshness(0.85)
            .with_relevance(fact.confidence)
            .with_token_count(token_count)
        })
        .collect::<Vec<_>>();
    if let Some(result) = ahp_context_result(fact_items) {
        results.push(result);
    }

    if let Some(file_contents) = injected.file_contents {
        let file_items = file_contents
            .into_iter()
            .map(|file| {
                let token_count = estimate_tokens(&file.snippet);
                ContextItem::new(
                    uuid::Uuid::new_v4().to_string(),
                    ContextType::Resource,
                    file.snippet,
                )
                .with_source(file.path)
                .with_provenance("ahp_file_snippet")
                .with_priority(0.8)
                .with_trust(0.8)
                .with_freshness(0.8)
                .with_relevance(file.relevance_score)
                .with_token_count(token_count)
            })
            .collect::<Vec<_>>();
        if let Some(result) = ahp_context_result(file_items) {
            results.push(result);
        }
    }

    if let Some(summary) = injected.project_summary {
        let mut lines = vec![
            format!("Project: {}", summary.project_name),
            summary.structure_description,
        ];
        if let Some(language) = summary.language {
            lines.push(format!("Language: {language}"));
        }
        if let Some(key_files) = summary.key_files.filter(|files| !files.is_empty()) {
            lines.push(format!("Key files: {}", key_files.join(", ")));
        }
        let content = lines.join("\n");
        let token_count = estimate_tokens(&content);
        if let Some(result) = ahp_context_result(vec![ContextItem::new(
            uuid::Uuid::new_v4().to_string(),
            ContextType::Resource,
            content,
        )
        .with_source("ahp://project-summary")
        .with_provenance("ahp_project_summary")
        .with_priority(0.7)
        .with_trust(0.75)
        .with_freshness(0.8)
        .with_relevance(0.9)
        .with_token_count(token_count)])
        {
            results.push(result);
        }
    }

    if let Some(knowledge) = injected.knowledge {
        let knowledge_items = knowledge
            .into_iter()
            .map(|content| {
                let token_count = estimate_tokens(&content);
                ContextItem::new(
                    uuid::Uuid::new_v4().to_string(),
                    ContextType::Resource,
                    content,
                )
                .with_source("ahp://knowledge")
                .with_provenance("ahp_knowledge")
                .with_priority(0.55)
                .with_trust(0.65)
                .with_freshness(0.6)
                .with_relevance(0.8)
                .with_token_count(token_count)
            })
            .collect::<Vec<_>>();
        if let Some(result) = ahp_context_result(knowledge_items) {
            results.push(result);
        }
    }

    if let Some(suggestions) = injected.suggestions.filter(|items| !items.is_empty()) {
        let content = format!("Harness suggestions:\n- {}", suggestions.join("\n- "));
        let token_count = estimate_tokens(&content);
        if let Some(result) = ahp_context_result(vec![ContextItem::new(
            uuid::Uuid::new_v4().to_string(),
            ContextType::Resource,
            content,
        )
        .with_source("ahp://suggestions")
        .with_provenance("ahp_suggestions")
        .with_priority(0.45)
        .with_trust(0.6)
        .with_freshness(0.8)
        .with_relevance(0.7)
        .with_token_count(token_count)])
        {
            results.push(result);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_detection_extracts_intent_and_target() {
        let event = detect_local_context_perception_intent(
            "find the file `src/agent.rs`",
            "s1",
            "/workspace",
        )
        .unwrap();

        assert_eq!(event.intent, "locate");
        assert_eq!(event.target_type, "file");
        assert_eq!(event.target_name, "src/agent.rs");
        assert_eq!(event.domain, "rust");
    }

    #[test]
    fn harness_intent_hints_override_local_fallbacks() {
        let event = build_pre_context_perception_from_intent(
            IntentDetectionResult {
                detected_intent: "validate".to_string(),
                confidence: 0.9,
                target_hints: Some(TargetHints {
                    target_type: Some("module".to_string()),
                    target_name: Some("runtime".to_string()),
                    domain: Some("agent".to_string()),
                }),
            },
            "debug runtime",
            "s2",
            "/workspace",
        );

        assert_eq!(event.intent, "validate");
        assert_eq!(event.target_type, "module");
        assert_eq!(event.target_name, "runtime");
        assert_eq!(event.domain, "agent");
    }

    #[test]
    fn language_hint_detects_cjk_prompts() {
        assert_eq!(detect_language_hint("继续优化"), Some("zh".to_string()));
        assert_eq!(detect_language_hint("plain ascii"), None);
    }
}
