//! Built-in skills
//!
//! Provides a set of default skills that are compatible with Claude Code format.
//!
//! Built-in skills are loaded from the `core/skills/` directory and embedded at compile time.

use super::Skill;
use std::sync::Arc;

const CODE_SEARCH_SKILL: &str = include_str!("../../skills/code-search.md");
const CODE_REVIEW_SKILL: &str = include_str!("../../skills/code-review.md");
const EXPLAIN_CODE_SKILL: &str = include_str!("../../skills/explain-code.md");
const FIND_BUGS_SKILL: &str = include_str!("../../skills/find-bugs.md");

/// Get all built-in skills
///
/// Returns a vector of Arc-wrapped skills that are ready to use.
pub fn builtin_skills() -> Vec<Arc<Skill>> {
    vec![
        // Code assistance skills
        Arc::new(code_search_skill()),
        Arc::new(code_review_skill()),
        Arc::new(explain_code_skill()),
        Arc::new(find_bugs_skill()),
    ]
}

fn parse_embedded_skill(content: &str) -> Skill {
    Skill::parse(content).expect("embedded builtin skill must be valid markdown with frontmatter")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillKind;

    #[test]
    fn test_builtin_skills_count() {
        let skills = builtin_skills();
        assert_eq!(
            skills.len(),
            4,
            "Expected 4 built-in code assistance skills"
        );
    }

    #[test]
    fn test_code_search_skill() {
        let skill = code_search_skill();
        assert_eq!(skill.name, "code-search");
        assert!(skill.content.contains("Code Search"));
        assert!(skill.allowed_tools.is_none());
        assert!(skill.is_tool_allowed("write"));
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
}
