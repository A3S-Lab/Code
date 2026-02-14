// Prompt Registry
//
// Central registry for all system prompts and prompt templates used in A3S Code.
// Every LLM-facing prompt is externalized here as a compile-time `include_str!`
// so the full agentic design is visible in one place.
//
// Directory layout:
//   prompts/
//   ├── default_system_prompt.md    — Main agent system prompt
//   ├── subagent_explore.md         — Explore subagent system prompt
//   ├── subagent_plan.md            — Plan subagent system prompt
//   ├── subagent_title.md           — Title generation subagent prompt
//   ├── subagent_summary.md         — Summary generation subagent prompt
//   ├── complexity_system.md        — Task complexity analyzer (system)
//   ├── complexity_user.md          — Task complexity analyzer (user template)
//   ├── plan_system.md              — Plan creation (system)
//   ├── plan_user.md                — Plan creation (user template)
//   ├── goal_extract_system.md      — Goal extraction (system)
//   ├── goal_extract_user.md        — Goal extraction (user template)
//   ├── goal_check_system.md        — Goal achievement check (system)
//   ├── goal_check_user.md          — Goal achievement check (user template)
//   ├── context_compact.md          — Context compaction / summarization
//   ├── title_generate.md           — Session title generation
//   ├── structured_output.md        — Structured JSON output suffix
//   ├── llm_plan_system.md          — LLM planner: plan creation (JSON)
//   ├── llm_goal_extract_system.md  — LLM planner: goal extraction (JSON)
//   └── llm_goal_check_system.md    — LLM planner: goal achievement (JSON)

// ============================================================================
// Default System Prompt
// ============================================================================

/// Default system prompt for A3S Code agents
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/default_system_prompt.md");

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
// Agent Loop — Complexity Analysis
// ============================================================================

/// System prompt for task complexity classification
pub const COMPLEXITY_SYSTEM: &str = include_str!("../prompts/complexity_system.md");

/// User template for complexity analysis. Placeholder: `{task}`
pub const COMPLEXITY_USER: &str = include_str!("../prompts/complexity_user.md");

// ============================================================================
// Agent Loop — Plan Creation (text-parsed)
// ============================================================================

/// System prompt for plan creation
pub const PLAN_SYSTEM: &str = include_str!("../prompts/plan_system.md");

/// User template for plan creation. Placeholders: `{context}`, `{task}`
pub const PLAN_USER: &str = include_str!("../prompts/plan_user.md");

// ============================================================================
// Agent Loop — Goal Extraction (text-parsed)
// ============================================================================

/// System prompt for goal extraction
pub const GOAL_EXTRACT_SYSTEM: &str = include_str!("../prompts/goal_extract_system.md");

/// User template for goal extraction. Placeholder: `{task}`
pub const GOAL_EXTRACT_USER: &str = include_str!("../prompts/goal_extract_user.md");

// ============================================================================
// Agent Loop — Goal Achievement Check (text-parsed)
// ============================================================================

/// System prompt for goal achievement check
pub const GOAL_CHECK_SYSTEM: &str = include_str!("../prompts/goal_check_system.md");

/// User template for goal achievement check. Placeholders: `{goal}`, `{criteria}`, `{current_state}`
pub const GOAL_CHECK_USER: &str = include_str!("../prompts/goal_check_user.md");

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
pub const TITLE_GENERATE: &str = include_str!("../prompts/title_generate.md");

// ============================================================================
// Service — Structured Output
// ============================================================================

/// User template for structured JSON output. Placeholders: `{prompt}`, `{schema}`
pub const STRUCTURED_OUTPUT: &str = include_str!("../prompts/structured_output.md");

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
pub const PLAN_EXECUTE_GOAL: &str = "Goal: {goal}\n\nExecute the following plan step by step:\n{steps}";

/// Template for per-step execution prompt
pub const PLAN_EXECUTE_STEP: &str = "Execute step {step_num}: {description}";

/// Template for fallback plan step description
pub const PLAN_FALLBACK_STEP: &str = "Execute step {step_num} of the plan";

// ============================================================================
// Helper Functions
// ============================================================================

/// Get the default system prompt
pub fn get_default_system_prompt() -> String {
    DEFAULT_SYSTEM_PROMPT.to_string()
}

/// Get a system prompt with custom additions
pub fn get_system_prompt_with_context(additional_context: &str) -> String {
    format!(
        "{}\n\n## Additional Context\n\n{}",
        DEFAULT_SYSTEM_PROMPT, additional_context
    )
}

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
    fn test_default_prompt_not_empty() {
        let prompt = get_default_system_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("A3S Code"));
    }

    #[test]
    fn test_prompt_contains_key_sections() {
        let prompt = get_default_system_prompt();
        assert!(prompt.contains("Agentic Coding"));
        assert!(prompt.contains("Tool & Skill Usage"));
        assert!(prompt.contains("Skill Discovery"));
        assert!(prompt.contains("Best Practices"));
    }

    #[test]
    fn test_prompt_with_context() {
        let prompt = get_system_prompt_with_context("This is a test project");
        assert!(prompt.contains("A3S Code"));
        assert!(prompt.contains("Additional Context"));
        assert!(prompt.contains("This is a test project"));
    }

    #[test]
    fn test_all_prompts_loaded() {
        // Verify all prompts are non-empty at compile time
        assert!(!SUBAGENT_EXPLORE.is_empty());
        assert!(!SUBAGENT_PLAN.is_empty());
        assert!(!SUBAGENT_TITLE.is_empty());
        assert!(!SUBAGENT_SUMMARY.is_empty());
        assert!(!COMPLEXITY_SYSTEM.is_empty());
        assert!(!COMPLEXITY_USER.is_empty());
        assert!(!PLAN_SYSTEM.is_empty());
        assert!(!PLAN_USER.is_empty());
        assert!(!GOAL_EXTRACT_SYSTEM.is_empty());
        assert!(!GOAL_EXTRACT_USER.is_empty());
        assert!(!GOAL_CHECK_SYSTEM.is_empty());
        assert!(!GOAL_CHECK_USER.is_empty());
        assert!(!CONTEXT_COMPACT.is_empty());
        assert!(!TITLE_GENERATE.is_empty());
        assert!(!STRUCTURED_OUTPUT.is_empty());
        assert!(!LLM_PLAN_SYSTEM.is_empty());
        assert!(!LLM_GOAL_EXTRACT_SYSTEM.is_empty());
        assert!(!LLM_GOAL_CHECK_SYSTEM.is_empty());
    }

    #[test]
    fn test_render_template() {
        let result = render(COMPLEXITY_USER, &[("task", "Write hello world")]);
        assert!(result.contains("Write hello world"));
        assert!(!result.contains("{task}"));
    }

    #[test]
    fn test_render_multiple_placeholders() {
        let result = render(
            GOAL_CHECK_USER,
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
}
