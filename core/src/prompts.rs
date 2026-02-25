// Prompt Registry
//
// Central registry for all system prompts and prompt templates used in A3S Code.
// Every LLM-facing prompt is externalized here as a compile-time `include_str!`
// so the full agentic design is visible in one place.
//
// Directory layout:
//   prompts/
//   ├── subagent_explore.md         — Explore subagent system prompt
//   ├── subagent_plan.md            — Plan subagent system prompt
//   ├── subagent_title.md           — Title generation subagent prompt
//   ├── subagent_summary.md         — Summary generation subagent prompt
//   ├── context_compact.md          — Context compaction / summarization
//   ├── title_generate.md           — Session title generation
//   ├── llm_plan_system.md          — LLM planner: plan creation (JSON)
//   ├── llm_goal_extract_system.md  — LLM planner: goal extraction (JSON)
//   └── llm_goal_check_system.md    — LLM planner: goal achievement (JSON)

// ============================================================================
// Default System Prompt
// ============================================================================

/// Default agentic system prompt — injected when no system prompt is configured.
///
/// Instructs the LLM to behave as an autonomous coding agent: use tools to act,
/// verify results, and keep working until the task is fully complete.
pub const SYSTEM_DEFAULT: &str = include_str!("../prompts/system_default.md");

/// Continuation message — injected as a user turn when the LLM stops without
/// completing the task (i.e. stops calling tools mid-task).
pub const CONTINUATION: &str = include_str!("../prompts/continuation.md");

// ============================================================================
// Subagent Prompts
// ============================================================================

/// Explore subagent — read-only codebase exploration
pub const SUBAGENT_EXPLORE: &str = include_str!("../prompts/subagent_explore.md");

/// Plan subagent — read-only planning and analysis
pub const SUBAGENT_PLAN: &str = include_str!("../prompts/subagent_plan.md");

/// Title subagent — generate concise conversation title
pub const SUBAGENT_TITLE: &str = include_str!("../prompts/subagent_title.md");

/// Summary subagent — summarize conversation key points
pub const SUBAGENT_SUMMARY: &str = include_str!("../prompts/subagent_summary.md");

// ============================================================================
// Session — Context Compaction
// ============================================================================

/// User template for context compaction. Placeholder: `{conversation}`
pub const CONTEXT_COMPACT: &str = include_str!("../prompts/context_compact.md");

/// Prefix for compacted summary messages
pub const CONTEXT_SUMMARY_PREFIX: &str =
    "[Context Summary: The following is a summary of earlier conversation]\n\n";

// ============================================================================
// Session — Title Generation
// ============================================================================

/// User template for session title generation. Placeholder: `{conversation}`
#[allow(dead_code)]
pub const TITLE_GENERATE: &str = include_str!("../prompts/title_generate.md");

// ============================================================================
// LLM Planner — JSON-structured prompts
// ============================================================================

/// System prompt for LLM planner: plan creation (JSON output)
pub const LLM_PLAN_SYSTEM: &str = include_str!("../prompts/llm_plan_system.md");

/// System prompt for LLM planner: goal extraction (JSON output)
pub const LLM_GOAL_EXTRACT_SYSTEM: &str = include_str!("../prompts/llm_goal_extract_system.md");

/// System prompt for LLM planner: goal achievement check (JSON output)
pub const LLM_GOAL_CHECK_SYSTEM: &str = include_str!("../prompts/llm_goal_check_system.md");

// ============================================================================
// Plan Execution (inline templates — no file needed)
// ============================================================================

/// Template for initial plan execution message
pub const PLAN_EXECUTE_GOAL: &str =
    "Goal: {goal}\n\nExecute the following plan step by step:\n{steps}";

/// Template for per-step execution prompt
pub const PLAN_EXECUTE_STEP: &str = "Execute step {step_num}: {description}";

/// Template for fallback plan step description
pub const PLAN_FALLBACK_STEP: &str = "Execute step {step_num} of the plan";

/// Template for merging results from parallel step execution
pub const PLAN_PARALLEL_RESULTS: &str =
    "The following steps were executed in parallel:\n{results}\n\nContinue with the next steps.";

// ============================================================================
// System Prompt Slots
// ============================================================================

/// Slot-based system prompt customization.
///
/// Users can customize specific parts of the system prompt without overriding
/// the core agentic capabilities (tool usage, autonomous behavior, completion
/// criteria). The default agentic core is ALWAYS included.
///
/// ## Assembly Order
///
/// ```text
/// [role]            ← Custom identity/role (e.g. "You are a Python expert")
/// [CORE]            ← Always present: Core Behaviour + Tool Usage Strategy + Completion Criteria
/// [guidelines]      ← Custom coding rules / constraints
/// [response_style]  ← Custom response format (replaces default Response Format section)
/// [extra]           ← Freeform additional instructions
/// ```
#[derive(Debug, Clone, Default)]
pub struct SystemPromptSlots {
    /// Custom role/identity prepended before the core prompt.
    ///
    /// Example: "You are a senior Python developer specializing in FastAPI."
    /// When set, replaces the default "You are A3S Code, an expert AI coding agent" line.
    pub role: Option<String>,

    /// Custom coding guidelines appended after the core prompt sections.
    ///
    /// Example: "Always use type hints. Follow PEP 8. Prefer dataclasses over dicts."
    pub guidelines: Option<String>,

    /// Custom response style that replaces the default "Response Format" section.
    ///
    /// When `None`, the default response format is used.
    pub response_style: Option<String>,

    /// Freeform extra instructions appended at the very end.
    ///
    /// This is the backward-compatible slot: setting `system_prompt` in the old API
    /// maps to this field.
    pub extra: Option<String>,
}

/// The default role line in SYSTEM_DEFAULT that gets replaced when `role` slot is set.
const DEFAULT_ROLE_LINE: &str =
    "You are A3S Code, an expert AI coding agent. You operate in an agentic loop: you\nthink, use tools, observe results, and keep working until the task is fully complete.";

/// The default response format section.
const DEFAULT_RESPONSE_FORMAT: &str = "## Response Format

- During work: emit tool calls, no prose.
- On completion: one short paragraph summarising what changed and why.
- On genuine blockers: ask a single, specific question.";

impl SystemPromptSlots {
    /// Build the final system prompt by assembling slots around the core prompt.
    ///
    /// The core agentic behavior (Core Behaviour, Tool Usage Strategy, Completion
    /// Criteria) is always preserved. Users can only customize the edges.
    pub fn build(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Normalize line endings: strip \r so string matching works on Windows
        // where include_str! may produce \r\n if the file has CRLF endings.
        let system_default = SYSTEM_DEFAULT.replace('\r', "");

        // 1. Role: replace default role line or use default
        let core = if let Some(ref role) = self.role {
            let custom_role = format!(
                "{}. You operate in an agentic loop: you\nthink, use tools, observe results, and keep working until the task is fully complete.",
                role.trim_end_matches('.')
            );
            system_default.replace(DEFAULT_ROLE_LINE, &custom_role)
        } else {
            system_default
        };

        // 2. Core: strip the default response format section if custom one is provided
        let core = if self.response_style.is_some() {
            core.replace(DEFAULT_RESPONSE_FORMAT, "")
                .trim_end()
                .to_string()
        } else {
            core.trim_end().to_string()
        };

        parts.push(core);

        // 3. Custom response style (replaces default Response Format)
        if let Some(ref style) = self.response_style {
            parts.push(format!("## Response Format\n\n{}", style));
        }

        // 4. Guidelines
        if let Some(ref guidelines) = self.guidelines {
            parts.push(format!("## Guidelines\n\n{}", guidelines));
        }

        // 5. Extra (freeform, backward-compatible with old system_prompt)
        if let Some(ref extra) = self.extra {
            parts.push(extra.clone());
        }

        parts.join("\n\n")
    }

    /// Create slots from a legacy full system prompt string.
    ///
    /// For backward compatibility: the entire string is placed in the `extra` slot,
    /// and the default core prompt is still prepended.
    pub fn from_legacy(prompt: String) -> Self {
        Self {
            extra: Some(prompt),
            ..Default::default()
        }
    }

    /// Returns true if all slots are empty (use pure default prompt).
    pub fn is_empty(&self) -> bool {
        self.role.is_none()
            && self.guidelines.is_none()
            && self.response_style.is_none()
            && self.extra.is_none()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Render a template by replacing `{key}` placeholders with values
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{}}}", key), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_prompts_loaded() {
        // Verify all prompts are non-empty at compile time
        assert!(!SYSTEM_DEFAULT.is_empty());
        assert!(!CONTINUATION.is_empty());
        assert!(!SUBAGENT_EXPLORE.is_empty());
        assert!(!SUBAGENT_PLAN.is_empty());
        assert!(!SUBAGENT_TITLE.is_empty());
        assert!(!SUBAGENT_SUMMARY.is_empty());
        assert!(!CONTEXT_COMPACT.is_empty());
        assert!(!TITLE_GENERATE.is_empty());
        assert!(!LLM_PLAN_SYSTEM.is_empty());
        assert!(!LLM_GOAL_EXTRACT_SYSTEM.is_empty());
        assert!(!LLM_GOAL_CHECK_SYSTEM.is_empty());
    }

    #[test]
    fn test_render_template() {
        let result = render(
            PLAN_EXECUTE_GOAL,
            &[("goal", "Build app"), ("steps", "1. Init")],
        );
        assert!(result.contains("Build app"));
        assert!(!result.contains("{goal}"));
    }

    #[test]
    fn test_render_multiple_placeholders() {
        let template = "Goal: {goal}\nCriteria: {criteria}\nState: {current_state}";
        let result = render(
            template,
            &[
                ("goal", "Build a REST API"),
                ("criteria", "- Endpoint works\n- Tests pass"),
                ("current_state", "API is deployed"),
            ],
        );
        assert!(result.contains("Build a REST API"));
        assert!(result.contains("Endpoint works"));
        assert!(result.contains("API is deployed"));
    }

    #[test]
    fn test_subagent_prompts_contain_guidelines() {
        assert!(SUBAGENT_EXPLORE.contains("Guidelines"));
        assert!(SUBAGENT_EXPLORE.contains("read-only"));
        assert!(SUBAGENT_PLAN.contains("Guidelines"));
        assert!(SUBAGENT_PLAN.contains("read-only"));
    }

    #[test]
    fn test_context_summary_prefix() {
        assert!(CONTEXT_SUMMARY_PREFIX.contains("Context Summary"));
    }

    // ── SystemPromptSlots tests ──

    #[test]
    fn test_slots_default_builds_system_default() {
        let slots = SystemPromptSlots::default();
        let built = slots.build();
        assert!(built.contains("Core Behaviour"));
        assert!(built.contains("Tool Usage Strategy"));
        assert!(built.contains("Completion Criteria"));
        assert!(built.contains("Response Format"));
        assert!(built.contains("A3S Code"));
    }

    #[test]
    fn test_slots_custom_role_replaces_default() {
        let slots = SystemPromptSlots {
            role: Some("You are a senior Python developer".to_string()),
            ..Default::default()
        };
        let built = slots.build();
        assert!(built.contains("You are a senior Python developer"));
        assert!(!built.contains("You are A3S Code"));
        // Core sections still present
        assert!(built.contains("Core Behaviour"));
        assert!(built.contains("Tool Usage Strategy"));
    }

    #[test]
    fn test_slots_custom_guidelines_appended() {
        let slots = SystemPromptSlots {
            guidelines: Some("Always use type hints. Follow PEP 8.".to_string()),
            ..Default::default()
        };
        let built = slots.build();
        assert!(built.contains("## Guidelines"));
        assert!(built.contains("Always use type hints"));
        assert!(built.contains("Core Behaviour"));
    }

    #[test]
    fn test_slots_custom_response_style_replaces_default() {
        let slots = SystemPromptSlots {
            response_style: Some("Be concise. Use bullet points.".to_string()),
            ..Default::default()
        };
        let built = slots.build();
        assert!(built.contains("Be concise. Use bullet points."));
        // Default response format content should be gone
        assert!(!built.contains("emit tool calls, no prose"));
        // But core is still there
        assert!(built.contains("Core Behaviour"));
    }

    #[test]
    fn test_slots_extra_appended() {
        let slots = SystemPromptSlots {
            extra: Some("Remember: always write tests first.".to_string()),
            ..Default::default()
        };
        let built = slots.build();
        assert!(built.contains("Remember: always write tests first."));
        assert!(built.contains("Core Behaviour"));
    }

    #[test]
    fn test_slots_from_legacy() {
        let slots = SystemPromptSlots::from_legacy("You are a helpful assistant.".to_string());
        let built = slots.build();
        // Legacy prompt goes into extra, core is still present
        assert!(built.contains("You are a helpful assistant."));
        assert!(built.contains("Core Behaviour"));
        assert!(built.contains("Tool Usage Strategy"));
    }

    #[test]
    fn test_slots_all_slots_combined() {
        let slots = SystemPromptSlots {
            role: Some("You are a Rust expert".to_string()),
            guidelines: Some("Use clippy. No unwrap.".to_string()),
            response_style: Some("Short answers only.".to_string()),
            extra: Some("Project uses tokio.".to_string()),
        };
        let built = slots.build();
        assert!(built.contains("You are a Rust expert"));
        assert!(built.contains("Core Behaviour"));
        assert!(built.contains("## Guidelines"));
        assert!(built.contains("Use clippy"));
        assert!(built.contains("Short answers only"));
        assert!(built.contains("Project uses tokio"));
        // Default response format replaced
        assert!(!built.contains("emit tool calls, no prose"));
    }

    #[test]
    fn test_slots_is_empty() {
        assert!(SystemPromptSlots::default().is_empty());
        assert!(!SystemPromptSlots {
            role: Some("test".to_string()),
            ..Default::default()
        }
        .is_empty());
    }
}
