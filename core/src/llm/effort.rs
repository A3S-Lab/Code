//! Provider effort parameters.
//!
//! Effort selects how deeply one completion thinks. It is not a tool-round
//! or step budget. Each provider receives its own parameter. `max_tokens`
//! stays large enough that thinking does not consume the visible answer.

/// Output window used at every effort level.
///
/// Effort changes how deeply the model thinks. It does not shrink the answer.
/// Anthropic counts thinking toward `max_tokens`, so a short window truncates
/// the turn once reasoning starts. A coding turn also needs room for a patch.
pub(crate) const HIGH_EFFORT_OUTPUT_TOKENS: usize = 65_536;

/// Host safety ceiling for tool rounds. Every effort level uses this same cap.
pub const TOOL_ROUND_SAFETY_CEILING: usize = 3_200;

pub(crate) fn normalize_effort(token: &str) -> &str {
    token.trim()
}

/// Anthropic `output_config.effort`.
pub(crate) fn anthropic_effort(token: &str) -> Option<&'static str> {
    match normalize_effort(token) {
        "none" | "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" | "ultracode" => Some("max"),
        _ => None,
    }
}

/// OpenAI `reasoning_effort`. `max` is not an OpenAI value.
pub(crate) fn openai_reasoning_effort(token: &str) -> Option<&'static str> {
    match normalize_effort(token) {
        "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" | "ultracode" => Some("xhigh"),
        _ => None,
    }
}

/// GLM `reasoning_effort` (`low` / `high` / `max`).
pub(crate) fn glm_reasoning_effort(token: &str) -> Option<&'static str> {
    match normalize_effort(token) {
        "none" | "minimal" | "low" => Some("low"),
        "medium" | "high" => Some("high"),
        "xhigh" | "max" | "ultracode" => Some("max"),
        _ => None,
    }
}

/// Minimum `max_tokens` so any effort level can still write a full answer.
pub(crate) fn output_token_floor(level: &str) -> usize {
    match level {
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            HIGH_EFFORT_OUTPUT_TOKENS
        }
        _ => 0,
    }
}

pub(crate) fn lifted_max_tokens(current: usize, level: &str) -> usize {
    current.max(output_token_floor(level))
}

pub(crate) fn is_glm_model(model: &str) -> bool {
    let id = model.rsplit(['/', ':']).next().unwrap_or(model);
    let id = id.trim().to_ascii_lowercase();
    id.starts_with("glm-5.2")
        || id.starts_with("glm-5.3")
        || id.starts_with("glm-5-2")
        || id.starts_with("glm-5-3")
        || id.starts_with("glm-")
        || id.starts_with("glm")
}

pub(crate) fn is_openai_reasoning_model(model: &str) -> bool {
    let id = model.rsplit(['/', ':']).next().unwrap_or(model);
    let id = id.trim().to_ascii_lowercase();
    id.starts_with("gpt-5")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("codex")
        || id.contains("-codex")
}

/// Menu values the model actually accepts. Empty when effort is prompt-only.
pub fn native_effort_menu(model_id: &str) -> &'static [&'static str] {
    if !model_sends_native_effort(model_id) {
        return &[];
    }
    let id = model_id.trim().to_ascii_lowercase();
    let (provider, name) = match id.split_once('/') {
        Some((provider, name)) => (provider, name),
        None => ("", id.as_str()),
    };
    if matches!(provider, "glm" | "zhipu" | "bigmodel") || is_glm_model(name) {
        return &["low", "high", "max"];
    }
    &["low", "medium", "high", "xhigh", "max"]
}

/// Whether this model id receives a native effort parameter instead of a
/// prompt-only depth guideline.
pub fn model_sends_native_effort(model_id: &str) -> bool {
    let id = model_id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return false;
    }
    let (provider, name) = match id.split_once('/') {
        Some((provider, name)) => (provider, name),
        None => ("", id.as_str()),
    };
    matches!(
        provider,
        "anthropic" | "claude" | "cc-switch" | "glm" | "zhipu" | "bigmodel"
    ) || is_glm_model(name)
        || name.starts_with("claude")
        || is_openai_reasoning_model(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_receive_their_own_effort_values() {
        assert_eq!(anthropic_effort("high"), Some("high"));
        assert_eq!(anthropic_effort("max"), Some("max"));
        assert_eq!(anthropic_effort("minimal"), Some("low"));
        assert_eq!(openai_reasoning_effort("max"), Some("xhigh"));
        assert_eq!(openai_reasoning_effort("none"), Some("none"));
        assert_eq!(glm_reasoning_effort("medium"), Some("high"));
        assert_eq!(glm_reasoning_effort("xhigh"), Some("max"));
        assert_eq!(anthropic_effort("nope"), None);
    }

    #[test]
    fn high_effort_lifts_the_output_window() {
        assert_eq!(lifted_max_tokens(8_192, "high"), HIGH_EFFORT_OUTPUT_TOKENS);
        assert_eq!(lifted_max_tokens(8_192, "low"), HIGH_EFFORT_OUTPUT_TOKENS);
        assert_eq!(
            lifted_max_tokens(8_192, "medium"),
            HIGH_EFFORT_OUTPUT_TOKENS
        );
        assert_eq!(lifted_max_tokens(128_000, "max"), 128_000);
    }

    #[test]
    fn native_effort_follows_the_model() {
        assert!(model_sends_native_effort("anthropic/claude-opus-4-6"));
        assert!(model_sends_native_effort("openai/gpt-5.4"));
        assert!(model_sends_native_effort("zhipu/glm-5.3"));
        assert!(!model_sends_native_effort("ollama/llama3"));
        assert!(!model_sends_native_effort("openai/gpt-4o"));
    }

    #[test]
    fn effort_menu_lists_only_legal_values() {
        assert_eq!(
            native_effort_menu("anthropic/claude-opus-4-6"),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(native_effort_menu("zhipu/glm-5"), ["low", "high", "max"]);
        assert!(native_effort_menu("ollama/llama3").is_empty());
        assert!(native_effort_menu("openai/gpt-4o").is_empty());
    }
}
