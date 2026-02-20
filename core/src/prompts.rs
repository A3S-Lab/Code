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
}
