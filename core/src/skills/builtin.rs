//! Built-in skills
//!
//! A3S Code no longer ships default embedded skills. This module keeps the
//! public `builtin_skills()` compatibility entry point so existing SDK callers
//! that query or enable built-ins continue to compile, but it intentionally
//! returns an empty list.

use super::Skill;
use std::sync::Arc;

/// Get all built-in skills.
///
/// Returns an empty list because built-in skills were removed. Load reusable
/// skills from `skill_dirs`, inline skills, or an explicit `SkillRegistry`.
pub fn builtin_skills() -> Vec<Arc<Skill>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skills_are_empty() {
        let skills = builtin_skills();
        assert!(skills.is_empty());
    }
}
