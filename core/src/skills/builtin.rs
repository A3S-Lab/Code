//! Built-in skills
//!
//! Provides a set of default skills that are compatible with Claude Code format.
//!
//! Built-in skills are loaded from the `core/skills/` directory and embedded at compile time.

use super::Skill;
use std::sync::Arc;

const AGENTIC_SEARCH_SKILL: &str = include_str!("../../skills/agentic-search.md");
const AGENTIC_PARSE_SKILL: &str = include_str!("../../skills/agentic-parse.md");
const CODE_SEARCH_SKILL: &str = include_str!("../../skills/code-search.md");
const CODE_REVIEW_SKILL: &str = include_str!("../../skills/code-review.md");
const EXPLAIN_CODE_SKILL: &str = include_str!("../../skills/explain-code.md");
const FIND_BUGS_SKILL: &str = include_str!("../../skills/find-bugs.md");
const BUILTIN_TOOLS_SKILL: &str = include_str!("../../skills/builtin-tools.md");
const DELEGATE_TASK_SKILL: &str = include_str!("../../skills/delegate-task.md");
const FIND_SKILLS_SKILL: &str = include_str!("../../skills/find-skills.md");

/// Get all built-in skills
///
/// Returns a vector of Arc-wrapped skills that are ready to use.
pub fn builtin_skills() -> Vec<Arc<Skill>> {
    vec![
        // Code assistance skills
        Arc::new(agentic_search_skill()),
        Arc::new(agentic_parse_skill()),
        Arc::new(code_search_skill()),
        Arc::new(code_review_skill()),
        Arc::new(explain_code_skill()),
        Arc::new(find_bugs_skill()),
        // Tool documentation skills
        Arc::new(builtin_tools_skill()),
        Arc::new(delegate_task_skill()),
        Arc::new(find_skills_skill()),
    ]
}

fn parse_embedded_skill(content: &str) -> Skill {
    Skill::parse(content).expect("embedded builtin skill must be valid markdown with frontmatter")
}

/// Agentic search skill
fn agentic_search_skill() -> Skill {
    parse_embedded_skill(AGENTIC_SEARCH_SKILL)
}

/// Agentic parse skill
fn agentic_parse_skill() -> Skill {
    parse_embedded_skill(AGENTIC_PARSE_SKILL)
}

/// Code search skill
fn code_search_skill() -> Skill {
    parse_embedded_skill(CODE_SEARCH_SKILL)
}

/// Code review skill
fn code_review_skill() -> Skill {
    parse_embedded_skill(CODE_REVIEW_SKILL)
}

/// Explain code skill
fn explain_code_skill() -> Skill {
    parse_embedded_skill(EXPLAIN_CODE_SKILL)
}

/// Find bugs skill
fn find_bugs_skill() -> Skill {
    parse_embedded_skill(FIND_BUGS_SKILL)
}

/// Built-in tools documentation skill
fn builtin_tools_skill() -> Skill {
    parse_embedded_skill(BUILTIN_TOOLS_SKILL)
}

/// Delegate task skill
fn delegate_task_skill() -> Skill {
    parse_embedded_skill(DELEGATE_TASK_SKILL)
}

/// Find skills skill
fn find_skills_skill() -> Skill {
    parse_embedded_skill(FIND_SKILLS_SKILL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillKind;

    #[test]
    fn test_builtin_skills_count() {
        let skills = builtin_skills();
        assert_eq!(
            skills.len(),
            9,
            "Expected 9 built-in skills (6 code assistance + 3 tool documentation)"
        );
    }

    #[test]
    fn test_agentic_search_skill() {
        let skill = agentic_search_skill();
        assert_eq!(skill.name, "agentic-search");
        assert!(skill.content.contains("Agentic Search"));
        assert!(skill.is_tool_allowed("agentic_search"));
        assert!(skill.is_tool_allowed("grep"));
        assert!(skill.is_tool_allowed("read"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_code_search_skill() {
        let skill = code_search_skill();
        assert_eq!(skill.name, "code-search");
        assert!(skill.content.contains("Code Search"));
        assert!(skill.is_tool_allowed("grep"));
        assert!(skill.is_tool_allowed("read"));
        assert!(skill.is_tool_allowed("glob"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_agentic_parse_skill() {
        let skill = agentic_parse_skill();
        assert_eq!(skill.name, "agentic-parse");
        assert!(skill.content.contains("document intelligence assistant"));
        assert!(skill.is_tool_allowed("agentic_parse"));
        assert!(skill.is_tool_allowed("read"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_code_review_skill() {
        let skill = code_review_skill();
        assert_eq!(skill.name, "code-review");
        assert!(skill.content.contains("Code Review"));
        assert_eq!(skill.kind, SkillKind::Instruction);
    }

    #[test]
    fn test_explain_code_skill() {
        let skill = explain_code_skill();
        assert_eq!(skill.name, "explain-code");
        assert!(skill.tags.contains(&"explain".to_string()));
    }

    #[test]
    fn test_find_bugs_skill() {
        let skill = find_bugs_skill();
        assert_eq!(skill.name, "find-bugs");
        assert!(skill.tags.contains(&"bugs".to_string()));
        assert!(skill.tags.contains(&"security".to_string()));
    }

    #[test]
    fn test_builtin_tools_skill() {
        let skill = builtin_tools_skill();
        assert_eq!(skill.name, "builtin-tools");
        assert!(skill.content.contains("Built-in Tools"));
        assert!(skill.content.contains("read"));
        assert!(skill.content.contains("write"));
        assert!(skill.content.contains("bash"));
    }

    #[test]
    fn test_delegate_task_skill() {
        let skill = delegate_task_skill();
        assert_eq!(skill.name, "delegate-task");
        assert!(skill.content.contains("Delegate Task"));
        assert!(skill.is_tool_allowed("task"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_find_skills_skill() {
        let skill = find_skills_skill();
        assert_eq!(skill.name, "find-skills");
        assert!(skill.content.contains("Find Skills"));
        assert!(skill.is_tool_allowed("search_skills"));
        assert!(skill.is_tool_allowed("install_skill"));
        assert!(skill.is_tool_allowed("load_skill"));
    }
}
