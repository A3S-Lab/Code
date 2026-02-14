// System Prompt Module
//
// Provides default system prompts for A3S Code agents.

/// Default system prompt for A3S Code agents
///
/// This prompt enables:
/// - Agentic coding capabilities
/// - Skill discovery and invocation
/// - Tool usage best practices
/// - Clear communication patterns
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/default_system_prompt.md");

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
}
