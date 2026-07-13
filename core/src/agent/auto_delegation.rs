use super::{AgentEvent, AgentLoop};
use crate::subagent::{AgentDefinition, AgentRegistry};
use anyhow::Result;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SYNTHESIS_CONTINUATION_MARKER: &str = "[synthesis]";

#[derive(Debug, Clone)]
struct AutoDelegationTask {
    agent: String,
    description: String,
    prompt: String,
    confidence: f32,
}

#[derive(Debug, Clone)]
struct AutoDelegationPlan {
    tasks: Vec<AutoDelegationTask>,
    reason: String,
}

pub(super) struct AutoDelegationOutcome {
    pub prompt: String,
    pub tool_calls_count: usize,
}

impl AgentLoop {
    pub(super) async fn maybe_apply_auto_delegation(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Result<Option<AutoDelegationOutcome>> {
        if !prompt_allows_auto_delegation(prompt) {
            return Ok(None);
        }
        let Some(plan) = self.build_auto_delegation_plan(prompt) else {
            return Ok(None);
        };

        let tool_name = if plan.tasks.len() == 1 {
            "task"
        } else {
            "parallel_task"
        };
        let args = if tool_name == "task" {
            task_to_args(&plan.tasks[0])
        } else {
            json!({
                "tasks": plan.tasks.iter().map(task_to_args).collect::<Vec<_>>()
            })
        };

        let (output, _exit_code, is_error, metadata) = self
            .execute_delegated_plan_tool(tool_name, &args, session_id, event_tx, cancel_token)
            .await;

        let envelope = json!({
            "type": "auto_delegation_results",
            "tool": tool_name,
            "reason": plan.reason,
            "failed": is_error,
            "tasks": plan.tasks.iter().map(|task| {
                json!({
                    "agent": task.agent,
                    "description": task.description,
                    "confidence": task.confidence,
                })
            }).collect::<Vec<_>>(),
            "metadata": metadata,
            "output": output,
        });

        Ok(Some(AutoDelegationOutcome {
            prompt: format!(
                "{prompt}\n\nAutomatic subagent context:\n{}\n\nUse the subagent findings as evidence, but make the final decision yourself.",
                serde_json::to_string(&envelope).unwrap_or_default()
            ),
            tool_calls_count: 1,
        }))
    }

    fn build_auto_delegation_plan(&self, prompt: &str) -> Option<AutoDelegationPlan> {
        let config = &self.config.auto_delegation;
        if !config.enabled || config.max_tasks == 0 {
            return None;
        }
        if !self.tool_executor.registry().contains("task") {
            return None;
        }
        let registry = self.config.agent_registry.as_ref()?;
        let explicit_target = explicit_agent_target(prompt, registry);
        let mut candidates = if let Some(target) = explicit_target.as_deref() {
            registry
                .get(target)
                .and_then(|agent| score_agent_for_prompt(&agent, prompt, Some(target)))
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            registry
                .list_visible()
                .into_iter()
                .filter(|agent| agent.name != "general")
                .filter_map(|agent| score_agent_for_prompt(&agent, prompt, None))
                .filter(|task| task.confidence >= config.min_confidence)
                .collect::<Vec<_>>()
        };

        candidates.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.agent.cmp(&right.agent))
        });
        candidates.dedup_by(|left, right| left.agent == right.agent);

        let max_tasks = config.max_tasks.min(self.config.max_parallel_tasks).max(1);
        let max_tasks = if config.auto_parallel { max_tasks } else { 1 };
        candidates.truncate(max_tasks);

        if candidates.is_empty() {
            return None;
        }
        if candidates.len() > 1 && !self.tool_executor.registry().contains("parallel_task") {
            candidates.truncate(1);
        }

        Some(AutoDelegationPlan {
            reason: if candidates.len() > 1 {
                "multiple high-confidence independent specialist matches".to_string()
            } else {
                "high-confidence specialist match".to_string()
            },
            tasks: candidates,
        })
    }
}

fn prompt_allows_auto_delegation(prompt: &str) -> bool {
    !prompt
        .trim_start()
        .starts_with(SYNTHESIS_CONTINUATION_MARKER)
}

fn task_to_args(task: &AutoDelegationTask) -> Value {
    json!({
        "agent": task.agent,
        "description": task.description,
        "prompt": task.prompt,
    })
}

fn score_agent_for_prompt(
    agent: &AgentDefinition,
    prompt: &str,
    explicit_target: Option<&str>,
) -> Option<AutoDelegationTask> {
    let lower_prompt = prompt.to_lowercase();
    let lower_description = agent.description.to_lowercase();
    let confidence = builtin_confidence(&agent.name, &lower_prompt)
        .max(explicit_agent_confidence(
            &agent.name,
            &lower_prompt,
            explicit_target,
        ))
        .max(description_confidence(&lower_description, &lower_prompt));

    (confidence > 0.0).then(|| AutoDelegationTask {
        agent: agent.name.clone(),
        description: auto_description(agent, prompt),
        prompt: auto_prompt(agent, prompt),
        confidence,
    })
}

fn explicit_agent_target(prompt: &str, registry: &AgentRegistry) -> Option<String> {
    let lower = prompt.to_lowercase();
    let mut aliases = registry
        .list()
        .into_iter()
        .flat_map(agent_explicit_aliases)
        .collect::<Vec<_>>();
    aliases.sort_by_key(|alias| std::cmp::Reverse(alias.0.len()));
    aliases.dedup();

    for (alias, target) in aliases {
        if contains_explicit_reference(&lower, &alias) {
            return Some(target);
        }
    }
    None
}

fn agent_explicit_aliases(agent: AgentDefinition) -> Vec<(String, String)> {
    let target = agent.name.clone();
    let normalized = target.to_ascii_lowercase();
    let mut names = vec![normalized.clone()];
    match normalized.as_str() {
        "general" => names.extend(
            [
                "general-purpose",
                "general_purpose",
                "generalpurpose",
                "general purpose",
            ]
            .into_iter()
            .map(str::to_string),
        ),
        "verification" => {
            names.extend(["verify", "verifier"].into_iter().map(str::to_string));
        }
        "review" => {
            names.extend(
                ["code-review", "code_reviewer", "reviewer"]
                    .into_iter()
                    .map(str::to_string),
            );
        }
        _ => {}
    }

    names
        .into_iter()
        .flat_map(|name| {
            let agent_prefixed = format!("agent-{name}");
            [
                (name.clone(), target.clone()),
                (agent_prefixed, target.clone()),
            ]
        })
        .collect()
}

fn contains_explicit_reference(prompt: &str, name: &str) -> bool {
    let direct_mentions = [
        format!("@{name}"),
        format!("@\"{name}"),
        format!("@'{name}"),
        format!("{name} (agent)"),
    ];
    if direct_mentions
        .iter()
        .any(|needle| contains_bounded(prompt, needle))
    {
        return true;
    }

    [
        format!("use {name} subagent"),
        format!("use the {name} subagent"),
        format!("use {name} agent"),
        format!("use the {name} agent"),
        format!("delegate to {name}"),
        format!("ask {name}"),
        format!("ask the {name}"),
        format!("使用{name}"),
        format!("使用 {name}"),
        format!("用{name}"),
        format!("用 {name}"),
    ]
    .iter()
    .any(|needle| contains_bounded(prompt, needle))
}

fn contains_bounded(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(idx, _)| {
        let before = text[..idx].chars().next_back();
        let after = text[idx + needle.len()..].chars().next();
        is_reference_boundary(before) && is_reference_boundary(after)
    })
}

fn is_reference_boundary(ch: Option<char>) -> bool {
    ch.map(|ch| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .unwrap_or(true)
}

fn agent_name_matches(agent_name: &str, candidate: &str) -> bool {
    let agent = agent_name.to_ascii_lowercase();
    let candidate = candidate.to_ascii_lowercase();
    agent == candidate
        || matches!(
            (agent.as_str(), candidate.as_str()),
            ("general", "general-purpose")
                | ("general", "general_purpose")
                | ("general", "generalpurpose")
                | ("verification", "verify")
                | ("review", "code-review")
                | ("review", "code_reviewer")
        )
}

fn builtin_confidence(name: &str, prompt: &str) -> f32 {
    match name {
        "explore"
            if contains_any(
                prompt,
                &[
                    "find", "search", "locate", "inspect", "查找", "搜索", "定位", "探索", "检查",
                ],
            ) =>
        {
            0.84
        }
        "plan"
            if contains_any(
                prompt,
                &["plan", "design", "architecture", "规划", "设计", "架构"],
            ) =>
        {
            0.82
        }
        "verification" if contains_verification_intent(prompt) => 0.88,
        "review"
            if contains_any(
                prompt,
                &[
                    "review",
                    "security",
                    "regression",
                    "risk",
                    "审查",
                    "评审",
                    "安全",
                    "风险",
                ],
            ) =>
        {
            0.86
        }
        "general"
            if contains_any(
                prompt,
                &[
                    "general-purpose",
                    "general purpose",
                    "general_purpose",
                    "multi-step",
                    "multistep",
                    "通用",
                    "多步骤",
                ],
            ) =>
        {
            0.83
        }
        _ => 0.0,
    }
}

fn explicit_agent_confidence(agent_name: &str, prompt: &str, explicit_target: Option<&str>) -> f32 {
    let name = agent_name.to_lowercase();
    if explicit_target
        .map(|target| agent_name_matches(agent_name, target))
        .unwrap_or(false)
        || contains_explicit_reference(prompt, &name)
    {
        0.96
    } else {
        0.0
    }
}

fn description_confidence(description: &str, prompt: &str) -> f32 {
    let shared_terms = significant_terms(description)
        .into_iter()
        .filter(|term| contains_intent_keyword(prompt, term))
        .count();
    let base: f32 = match shared_terms {
        0 => 0.0,
        1 => 0.62,
        2 => 0.74,
        _ => 0.80,
    };

    let proactive_match = description.contains("proactive")
        && [
            "change",
            "changed",
            "changes",
            "documentation",
            "docs",
            "fix",
            "implement",
            "test",
            "review",
            "verify",
            "修改",
            "变更",
            "修复",
            "实现",
            "测试",
            "审查",
            "验证",
        ]
        .iter()
        .any(|term| contains_intent_keyword(prompt, term));

    if proactive_match {
        (base.max(0.74) + 0.16_f32).min(0.9)
    } else if base > 0.0 {
        base
    } else {
        0.0
    }
}

fn significant_terms(text: &str) -> Vec<&str> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| term.len() >= 4)
        .filter(|term| {
            !matches!(
                *term,
                "agent"
                    | "task"
                    | "with"
                    | "that"
                    | "this"
                    | "from"
                    | "only"
                    | "read"
                    | "write"
                    | "code"
                    | "file"
                    | "files"
            )
        })
        .collect()
}

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

fn contains_verification_intent(prompt: &str) -> bool {
    [
        "测试",
        "验证",
        "回归",
        "test",
        "tests",
        "tested",
        "testing",
        "verify",
        "verifies",
        "verified",
        "verifying",
        "validate",
        "validates",
        "validated",
        "validating",
        "regression",
        "regressions",
        "smoke",
    ]
    .iter()
    .any(|term| contains_intent_keyword(prompt, term))
}

fn contains_intent_keyword(text: &str, keyword: &str) -> bool {
    if keyword.is_ascii() {
        contains_non_path_keyword(text, keyword)
    } else {
        text.contains(keyword)
    }
}

/// Match an intent keyword as a word while ignoring the same token when it is
/// visibly part of a path such as `test.md` or `tests/parser.rs`.
fn contains_non_path_keyword(text: &str, keyword: &str) -> bool {
    text.match_indices(keyword).any(|(start, _)| {
        let end = start + keyword.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if !is_reference_boundary(before) || !is_reference_boundary(after) {
            return false;
        }

        let after_dot = (after == Some('.'))
            .then(|| text[end + 1..].chars().next())
            .flatten();
        !matches!(before, Some('/' | '\\' | '.'))
            && !matches!(after, Some('/' | '\\'))
            && !after_dot.is_some_and(|ch| ch.is_ascii_alphanumeric())
    })
}

fn auto_description(agent: &AgentDefinition, prompt: &str) -> String {
    format!(
        "Auto-delegated to {} for: {}",
        agent.name,
        crate::text::truncate_utf8(prompt, 120)
    )
}

fn auto_prompt(agent: &AgentDefinition, prompt: &str) -> String {
    format!(
        "Parent request:\n{prompt}\n\nSpecialist role:\n{}\n\nReturn a compact result with summary, evidence, risks, and confidence. Do not ask follow-up questions.",
        agent.description
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::AgentDefinition;

    #[test]
    fn scores_builtin_review_and_verification_prompts() {
        let review = AgentDefinition::new("review", "Review code");
        let verify = AgentDefinition::new("verification", "Run tests");

        let review_score =
            score_agent_for_prompt(&review, "Review the current diff", None).unwrap();
        let verify_score = score_agent_for_prompt(&verify, "Run regression tests", None).unwrap();

        assert!(review_score.confidence >= 0.86);
        assert!(verify_score.confidence >= 0.88);
    }

    #[test]
    fn test_filename_does_not_trigger_verification_delegation() {
        let verify = AgentDefinition::new("verification", "Run tests");

        assert!(score_agent_for_prompt(&verify, "删除 test.md", None).is_none());
        assert!(score_agent_for_prompt(&verify, "Delete ./tests/test.md", None).is_none());
        assert!(score_agent_for_prompt(&verify, "Run tests for test.md", None).is_some());
    }

    #[test]
    fn synthesis_continuation_never_auto_delegates() {
        assert!(!prompt_allows_auto_delegation(
            "[synthesis]\nWrite the final answer without more subagents."
        ));
        assert!(prompt_allows_auto_delegation(
            "Review and verify the implementation."
        ));
    }

    #[test]
    fn description_matching_supports_custom_proactive_agents() {
        let agent = AgentDefinition::new(
            "docs-checker",
            "Use proactively after documentation changes to inspect docs consistency",
        );

        let scored =
            score_agent_for_prompt(&agent, "Fix documentation and test examples", None).unwrap();

        assert_eq!(scored.agent, "docs-checker");
        assert!(scored.confidence >= 0.8);
    }

    #[test]
    fn explicit_mentions_support_claude_style_agent_names() {
        let general = AgentDefinition::new("general", "General-purpose agent");

        let scored = score_agent_for_prompt(
            &general,
            "Use @general-purpose to handle the multi-step implementation",
            Some("general-purpose"),
        )
        .unwrap();

        assert_eq!(scored.agent, "general");
        assert!(scored.confidence >= 0.96);
    }

    #[test]
    fn explicit_target_supports_claude_agent_prefix_for_builtin_alias() {
        let registry = AgentRegistry::new();

        let target = explicit_agent_target("Use @agent-general-purpose for this change", &registry);

        assert_eq!(target.as_deref(), Some("general"));
    }

    #[test]
    fn explicit_target_supports_custom_agent_names() {
        let registry = AgentRegistry::new();
        registry.register(AgentDefinition::new(
            "docs-auditor",
            "Use proactively after documentation changes",
        ));

        let target = explicit_agent_target("Use the docs-auditor subagent", &registry);

        assert_eq!(target.as_deref(), Some("docs-auditor"));
    }

    #[test]
    fn explicit_target_supports_quoted_claude_agent_mentions() {
        let registry = AgentRegistry::new();
        registry.register(AgentDefinition::new(
            "code-reviewer",
            "Review implementation quality",
        ));

        let target = explicit_agent_target(r#"Ask @"code-reviewer (agent)" to review"#, &registry);

        assert_eq!(target.as_deref(), Some("code-reviewer"));
    }

    #[test]
    fn explicit_target_does_not_treat_plain_plan_usage_as_an_agent_request() {
        let registry = AgentRegistry::new();

        let target = explicit_agent_target("Use the plan from the previous message", &registry);

        assert!(target.is_none());
    }

    #[test]
    fn explicit_confidence_does_not_treat_plain_plan_usage_as_an_agent_request() {
        assert_eq!(
            explicit_agent_confidence("plan", "use the plan from earlier", None),
            0.0
        );
    }

    #[test]
    fn proactive_description_can_trigger_without_shared_terms() {
        let agent = AgentDefinition::new(
            "quality-reviewer",
            "Use proactively after code changes to review quality",
        );

        let scored = score_agent_for_prompt(&agent, "I changed the auth flow", None).unwrap();

        assert_eq!(scored.agent, "quality-reviewer");
        assert!(scored.confidence >= 0.8);
    }
}
