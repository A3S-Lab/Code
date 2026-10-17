//! Studio standing instructions admitted when an ACP session is built.
//!
//! User text stays the user's words. Saved instructions, review posture, and
//! the product language ride in `_meta.a3sHost` and land in [`SystemPromptSlots`].
//! Per-turn constraints ride in `_meta.a3sTurn` and prefix only that turn's
//! model string. Prompt text never grants a tool.

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{AgentStyle, SystemPromptSlots};

const GUIDELINES_LIMIT: usize = 8000;
const ROLE_LIMIT: usize = 500;
const DIRECTIVE_LIMIT: usize = 500;
const DIRECTIVE_COUNT_LIMIT: usize = 8;

/// Same sentence the Studio chat preface uses for subagent review.
const REVIEW_SUBAGENT: &str = "代码审查：在独立子会话中审查本次改动，不要把审查过程写进当前会话。";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StudioHost {
    /// Explicit experiment input; rendered only by Core's policy path.
    pub work_policy: Option<a3s_code_core::WorkPolicyV1>,
    /// Replaces the default "You are A3S Code" line when non-empty.
    pub role: String,
    pub guidelines: String,
    pub review_subagent: bool,
    /// A fixed source snapshot is supplied by the host; no tool may be exposed.
    pub source_review: bool,
    pub output_language: Option<String>,
    /// Absolute roots the agent rescans. The catalog array in meta is ignored.
    pub assets: Option<crate::local_assets::AssetRoots>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnContext {
    pub directives: Vec<String>,
    pub research: bool,
}

pub fn studio_host_from_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<StudioHost, String> {
    let work_policy = crate::engineering::policy_from_meta(meta)?;
    let Some(host) = meta
        .and_then(|meta| meta.get("a3sHost"))
        .and_then(|value| value.as_object())
    else {
        return Ok(StudioHost {
            work_policy,
            ..Default::default()
        });
    };
    let role = host
        .get("role")
        .and_then(|value| value.as_str())
        .map(sanitize_limited(ROLE_LIMIT))
        .unwrap_or_default();
    let guidelines = host
        .get("guidelines")
        .and_then(|value| value.as_str())
        .map(sanitize_limited(GUIDELINES_LIMIT))
        .unwrap_or_default();
    let review_subagent = host.get("review").and_then(|value| value.as_str()) == Some("subagent");
    let source_review = match host.get("sourceReview") {
        None => false,
        Some(serde_json::Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("a3sHost.sourceReview must be a boolean".into()),
    };
    let output_language = host
        .get("outputLanguage")
        .and_then(|value| value.as_str())
        .and_then(normalize_output_language);
    Ok(StudioHost {
        work_policy,
        role,
        guidelines,
        review_subagent,
        source_review,
        output_language,
        assets: crate::local_assets::parse_roots(host.get("assets")),
    })
}

pub fn turn_context_from_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> TurnContext {
    let Some(turn) = meta
        .and_then(|meta| meta.get("a3sTurn"))
        .and_then(|value| value.as_object())
    else {
        return TurnContext::default();
    };
    let mut directives = Vec::new();
    if let Some(items) = turn.get("directives").and_then(|value| value.as_array()) {
        for item in items.iter().take(DIRECTIVE_COUNT_LIMIT) {
            let Some(text) = item.as_str() else {
                continue;
            };
            let text = sanitize_limited(DIRECTIVE_LIMIT)(text);
            if !text.is_empty() {
                directives.push(text);
            }
        }
    }
    TurnContext {
        directives,
        research: turn
            .get("research")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    }
}

/// Merge standing host text into `extra` and pin `output_language`.
///
/// The effort budget in `guidelines` stays where the ACP agent wrote it.
pub fn merge_studio_host(mut slots: SystemPromptSlots, host: &StudioHost) -> SystemPromptSlots {
    if !host.role.is_empty() {
        slots.role = Some(host.role.clone());
    }
    append_extra(&mut slots, &host_extra(host));
    if let Some(roots) = &host.assets {
        let catalog = crate::local_assets::load_catalog(roots);
        append_extra(
            &mut slots,
            &crate::local_assets::catalog_prompt(&catalog, roots.wiki.is_some()),
        );
    }
    if let Some(language) = &host.output_language {
        slots.output_language = Some(language.clone());
    }
    slots
}

fn append_extra(slots: &mut SystemPromptSlots, body: &str) {
    if body.is_empty() {
        return;
    }
    slots.extra = Some(match slots.extra.take() {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{body}"),
        _ => body.to_string(),
    });
}

fn host_extra(host: &StudioHost) -> String {
    let mut lines = Vec::new();
    if !host.guidelines.is_empty() {
        lines.push(host.guidelines.clone());
    }
    if host.review_subagent {
        lines.push(REVIEW_SUBAGENT.to_string());
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("Host instructions:\n{}", lines.join("\n\n"))
}

/// Prefix per-turn constraints. An empty directive list leaves `user` unchanged.
pub fn apply_turn_directives(user: &str, turn: &TurnContext) -> String {
    if turn.directives.is_empty() {
        return user.to_string();
    }
    let lines = turn
        .directives
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Host turn constraints:\n{lines}\n\nUser request:\n{user}")
}

/// Read-only tool policy for one research turn. Caller denies still win.
pub fn research_policy(denies: &[String]) -> PermissionPolicy {
    let mut policy = a3s_code_core::permissions::specialty_permission_policy(AgentStyle::Explore)
        .unwrap_or_else(|| {
            let mut closed = PermissionPolicy::new().deny_all(&[
                "write",
                "edit",
                "download",
                "bash",
                "task",
                "parallel_task",
            ]);
            closed.default_decision = PermissionDecision::Deny;
            closed
        });
    // A selected local asset is a read. Caller denies still win.
    policy = policy.allow("open_local_asset").allow("search_wiki");
    for rule in denies {
        policy = policy.deny(rule);
    }
    policy
}

fn sanitize_limited(limit: usize) -> impl Fn(&str) -> String {
    move |raw| {
        let cleaned: String = raw.chars().filter(|ch| *ch != '\0').collect();
        cleaned.trim().chars().take(limit).collect()
    }
}

fn normalize_output_language(raw: &str) -> Option<String> {
    match raw.trim() {
        "zh-CN" => Some("zh-CN".to_string()),
        "en-US" => Some("en-US".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn missing_meta_is_a_no_op() {
        let host = studio_host_from_meta(None).unwrap();
        assert_eq!(host, StudioHost::default());
        let slots = merge_studio_host(
            SystemPromptSlots::default()
                .with_guidelines("Session budget: thinking_budget=1 tool_rounds=2."),
            &host,
        );
        assert!(slots.guidelines.unwrap().contains("thinking_budget=1"));
        assert!(slots.role.is_none());
        assert!(slots.extra.is_none());
        assert!(slots.output_language.is_none());
        assert_eq!(
            apply_turn_directives("你好", &TurnContext::default()),
            "你好"
        );
    }

    #[test]
    fn source_review_is_an_explicit_boolean_restriction() {
        for enabled in [false, true] {
            let parsed = meta(serde_json::json!({"a3sHost":{"sourceReview":enabled}}));
            assert_eq!(
                studio_host_from_meta(Some(&parsed)).unwrap().source_review,
                enabled
            );
        }
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!("true"),
            serde_json::json!(1),
        ] {
            let parsed = meta(serde_json::json!({"a3sHost":{"sourceReview":invalid}}));
            assert_eq!(
                studio_host_from_meta(Some(&parsed)).unwrap_err(),
                "a3sHost.sourceReview must be a boolean"
            );
        }
    }

    #[test]
    fn host_text_lands_in_extra_and_keeps_the_budget_line() {
        let parsed = meta(serde_json::json!({
            "a3sHost": {
                "guidelines": "每次都带上\u{0000}",
                "review": "subagent",
                "outputLanguage": "zh-CN"
            }
        }));
        let host = studio_host_from_meta(Some(&parsed)).unwrap();
        assert_eq!(host.guidelines, "每次都带上");
        assert!(host.review_subagent);
        let slots = merge_studio_host(
            SystemPromptSlots::default()
                .with_guidelines("Session budget: thinking_budget=8 tool_rounds=4.")
                .with_extra("kept"),
            &host,
        );
        let guidelines = slots.guidelines.unwrap();
        assert!(guidelines.contains("thinking_budget=8"));
        assert!(!guidelines.contains("每次都带上"));
        let extra = slots.extra.unwrap();
        assert!(extra.starts_with("kept\n\nHost instructions:\n"));
        assert!(extra.contains("每次都带上"));
        assert!(extra.contains(REVIEW_SUBAGENT));
        assert_eq!(slots.output_language.as_deref(), Some("zh-CN"));
        assert!(slots.role.is_none());
    }

    #[test]
    fn role_replaces_the_default_identity_line() {
        let parsed = meta(serde_json::json!({
            "a3sHost": { "role": "You are ToBot\u{0000}", "guidelines": "keep" }
        }));
        let host = studio_host_from_meta(Some(&parsed)).unwrap();
        assert_eq!(host.role, "You are ToBot");
        let slots = merge_studio_host(SystemPromptSlots::default(), &host);
        let built = slots.build();
        assert!(built.contains("You are ToBot"));
        assert!(!built.contains("You are A3S Code"));
        assert!(slots.extra.unwrap().contains("keep"));
    }

    #[test]
    fn inline_review_and_unknown_language_add_nothing() {
        let parsed = meta(serde_json::json!({
            "a3sHost": { "guidelines": "  ", "review": "inline", "outputLanguage": "fr-FR" },
            "other": true
        }));
        let host = studio_host_from_meta(Some(&parsed)).unwrap();
        let slots = merge_studio_host(SystemPromptSlots::default(), &host);
        assert!(slots.extra.is_none());
        assert!(slots.output_language.is_none());
    }

    #[test]
    fn guidelines_stop_at_eight_thousand_characters() {
        let long = "测".repeat(8001);
        let parsed = meta(serde_json::json!({ "a3sHost": { "guidelines": long } }));
        let host = studio_host_from_meta(Some(&parsed)).unwrap();
        assert_eq!(host.guidelines.chars().count(), 8000);
    }

    #[test]
    fn turn_directives_prefix_the_model_string_only() {
        let parsed = meta(serde_json::json!({
            "a3sTurn": {
                "directives": ["自动审查：完成前检查结论、引用和改动，并标出不确定之处。", "", 1],
                "research": true
            }
        }));
        let turn = turn_context_from_meta(Some(&parsed));
        assert!(turn.research);
        assert_eq!(turn.directives.len(), 1);
        let wrapped = apply_turn_directives("只看这一句", &turn);
        assert!(wrapped.starts_with("Host turn constraints:\n- 自动审查："));
        assert!(wrapped.ends_with("User request:\n只看这一句"));
        let quiet = TurnContext {
            directives: Vec::new(),
            research: true,
        };
        assert_eq!(apply_turn_directives("只看这一句", &quiet), "只看这一句");
    }

    #[test]
    fn research_denies_writes_and_keeps_caller_denies() {
        let policy = research_policy(&["Bash(cat:*)".to_string()]);
        assert_eq!(
            policy.check("write", &serde_json::json!({})),
            PermissionDecision::Deny
        );
        assert_eq!(
            policy.check("read", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        assert!(policy.deny.iter().any(|rule| rule.rule == "Bash(cat:*)"));
        assert_eq!(policy.default_decision, PermissionDecision::Deny);
    }
}
