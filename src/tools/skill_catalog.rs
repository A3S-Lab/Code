//! Skill Catalog System
//!
//! Provides catalog-mode injection for skills. When the number of instruction-kind
//! skills exceeds a threshold, a lightweight catalog is injected instead of full
//! skill content. The LLM can then load specific skills on-demand via `load_skill`.

use super::skill::{Skill, SkillKind};

/// Default threshold for switching to catalog mode.
/// When the number of instruction-kind skills exceeds this, a lightweight
/// catalog is injected instead of full content.
pub const DEFAULT_CATALOG_THRESHOLD: usize = 3;

/// Build a lightweight catalog prompt listing skill names and descriptions.
///
/// Only includes Instruction-kind skills. Tool-kind skills register tools
/// directly and don't need prompt injection.
pub fn build_catalog_prompt(skills: &[Skill]) -> String {
    let instructions: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.kind == SkillKind::Instruction && !s.content.is_empty())
        .collect();

    if instructions.is_empty() {
        return String::new();
    }

    let mut xml = String::from("\n\n<skill-catalog>\n<instructions>\n");
    xml.push_str(
        "The following skills are available. To use a skill, call `load_skill` with the skill name.\n\
         Only load skills relevant to the current task.\n"
    );
    xml.push_str("</instructions>\n<available-skills>\n");

    for skill in &instructions {
        let desc = if skill.description.is_empty() {
            "No description"
        } else {
            &skill.description
        };
        xml.push_str(&format!(
            "  <skill name=\"{}\">{}</skill>\n",
            skill.name, desc
        ));
    }

    xml.push_str("</available-skills>\n</skill-catalog>");
    xml
}

/// Build full skills prompt with complete content for each skill.
///
/// Only includes Instruction-kind skills with non-empty content.
pub fn build_full_skills_prompt(skills: &[Skill]) -> String {
    let instructions: Vec<&Skill> = skills
        .iter()
        .filter(|s| s.kind == SkillKind::Instruction && !s.content.is_empty())
        .collect();

    if instructions.is_empty() {
        return String::new();
    }

    let mut xml = String::from("\n\n<skills>\n");
    for skill in &instructions {
        xml.push_str(&format!(
            "<skill name=\"{}\">\n{}\n</skill>\n",
            skill.name, skill.content
        ));
    }
    xml.push_str("</skills>");
    xml
}

/// Build the skills injection for the system prompt.
///
/// Decides between full content and catalog mode based on the number of
/// instruction-kind skills relative to the threshold.
///
/// - `instruction_count <= threshold` → full content (existing behavior)
/// - `instruction_count > threshold` → lightweight catalog
pub fn build_skills_injection(skills: &[Skill], threshold: usize) -> String {
    let instruction_count = skills
        .iter()
        .filter(|s| s.kind == SkillKind::Instruction && !s.content.is_empty())
        .count();

    if instruction_count == 0 {
        return String::new();
    }

    if instruction_count <= threshold {
        build_full_skills_prompt(skills)
    } else {
        build_catalog_prompt(skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str, desc: &str, kind: SkillKind, content: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind,
            content: content.to_string(),
        }
    }

    #[test]
    fn test_build_catalog_prompt_empty() {
        let result = build_catalog_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_catalog_prompt_instruction_skills() {
        let skills = vec![
            make_skill("react", "React patterns", SkillKind::Instruction, "content"),
            make_skill("review", "Code review", SkillKind::Instruction, "content"),
        ];
        let result = build_catalog_prompt(&skills);
        assert!(result.contains("<skill-catalog>"));
        assert!(result.contains("load_skill"));
        assert!(result.contains("name=\"react\""));
        assert!(result.contains("React patterns"));
        assert!(result.contains("name=\"review\""));
        assert!(result.contains("Code review"));
        assert!(result.contains("</skill-catalog>"));
    }

    #[test]
    fn test_build_catalog_prompt_excludes_tool_kind() {
        let skills = vec![
            make_skill("guide", "A guide", SkillKind::Instruction, "content"),
            make_skill("my-tool", "A tool", SkillKind::Tool, "tool content"),
        ];
        let result = build_catalog_prompt(&skills);
        assert!(result.contains("name=\"guide\""));
        assert!(!result.contains("name=\"my-tool\""));
    }

    #[test]
    fn test_build_catalog_prompt_excludes_agent_kind() {
        let skills = vec![
            make_skill("guide", "A guide", SkillKind::Instruction, "content"),
            make_skill("my-agent", "An agent", SkillKind::Agent, "agent content"),
        ];
        let result = build_catalog_prompt(&skills);
        assert!(result.contains("name=\"guide\""));
        assert!(!result.contains("name=\"my-agent\""));
    }

    #[test]
    fn test_build_catalog_prompt_skips_empty_content() {
        let skills = vec![
            make_skill("empty", "Empty skill", SkillKind::Instruction, ""),
            make_skill("full", "Full skill", SkillKind::Instruction, "content"),
        ];
        let result = build_catalog_prompt(&skills);
        assert!(!result.contains("name=\"empty\""));
        assert!(result.contains("name=\"full\""));
    }

    #[test]
    fn test_build_catalog_prompt_no_description_fallback() {
        let skills = vec![make_skill("nodesc", "", SkillKind::Instruction, "content")];
        let result = build_catalog_prompt(&skills);
        assert!(result.contains("No description"));
    }

    #[test]
    fn test_build_full_skills_prompt_empty() {
        let result = build_full_skills_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_full_skills_prompt_includes_content() {
        let skills = vec![make_skill(
            "react",
            "React patterns",
            SkillKind::Instruction,
            "Use React hooks for state management.",
        )];
        let result = build_full_skills_prompt(&skills);
        assert!(result.contains("<skills>"));
        assert!(result.contains("name=\"react\""));
        assert!(result.contains("Use React hooks for state management."));
        assert!(result.contains("</skills>"));
    }

    #[test]
    fn test_build_full_skills_prompt_excludes_tool_kind() {
        let skills = vec![
            make_skill("guide", "A guide", SkillKind::Instruction, "guide content"),
            make_skill("my-tool", "A tool", SkillKind::Tool, "tool content"),
        ];
        let result = build_full_skills_prompt(&skills);
        assert!(result.contains("name=\"guide\""));
        assert!(!result.contains("name=\"my-tool\""));
    }

    #[test]
    fn test_build_skills_injection_empty() {
        let result = build_skills_injection(&[], 3);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_skills_injection_below_threshold_uses_full() {
        let skills = vec![
            make_skill("a", "Skill A", SkillKind::Instruction, "content a"),
            make_skill("b", "Skill B", SkillKind::Instruction, "content b"),
        ];
        let result = build_skills_injection(&skills, 3);
        // Below threshold → full mode with <skills> tag
        assert!(result.contains("<skills>"));
        assert!(result.contains("content a"));
        assert!(result.contains("content b"));
        assert!(!result.contains("<skill-catalog>"));
    }

    #[test]
    fn test_build_skills_injection_at_threshold_uses_full() {
        let skills = vec![
            make_skill("a", "A", SkillKind::Instruction, "ca"),
            make_skill("b", "B", SkillKind::Instruction, "cb"),
            make_skill("c", "C", SkillKind::Instruction, "cc"),
        ];
        let result = build_skills_injection(&skills, 3);
        // At threshold → still full mode
        assert!(result.contains("<skills>"));
        assert!(!result.contains("<skill-catalog>"));
    }

    #[test]
    fn test_build_skills_injection_above_threshold_uses_catalog() {
        let skills = vec![
            make_skill("a", "Skill A", SkillKind::Instruction, "ca"),
            make_skill("b", "Skill B", SkillKind::Instruction, "cb"),
            make_skill("c", "Skill C", SkillKind::Instruction, "cc"),
            make_skill("d", "Skill D", SkillKind::Instruction, "cd"),
        ];
        let result = build_skills_injection(&skills, 3);
        // Above threshold → catalog mode
        assert!(result.contains("<skill-catalog>"));
        assert!(result.contains("load_skill"));
        assert!(!result.contains("<skills>"));
    }

    #[test]
    fn test_build_skills_injection_tool_kind_not_counted() {
        // 2 instruction + 2 tool = only 2 instruction count, below threshold of 3
        let skills = vec![
            make_skill("a", "A", SkillKind::Instruction, "ca"),
            make_skill("b", "B", SkillKind::Instruction, "cb"),
            make_skill("t1", "Tool 1", SkillKind::Tool, "tool content 1"),
            make_skill("t2", "Tool 2", SkillKind::Tool, "tool content 2"),
        ];
        let result = build_skills_injection(&skills, 3);
        // Only 2 instruction skills → full mode
        assert!(result.contains("<skills>"));
        assert!(!result.contains("<skill-catalog>"));
        // Tool-kind should not appear in output
        assert!(!result.contains("name=\"t1\""));
        assert!(!result.contains("name=\"t2\""));
    }

    #[test]
    fn test_default_catalog_threshold() {
        assert_eq!(DEFAULT_CATALOG_THRESHOLD, 3);
    }

    #[test]
    fn test_build_skills_injection_only_tool_skills_returns_empty() {
        let skills = vec![
            make_skill("t1", "Tool 1", SkillKind::Tool, "tool content"),
            make_skill("t2", "Tool 2", SkillKind::Tool, "more tool content"),
        ];
        let result = build_skills_injection(&skills, 3);
        assert!(result.is_empty());
    }
}
