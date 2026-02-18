//! Test skill system prompt auto-injection

use a3s_code_core::{
    skills::{Skill, SkillKind, SkillRegistry},
    Agent,
    SessionOptions,
};
use std::sync::Arc;

#[tokio::test]
async fn test_skill_system_prompt_injection() {
    // Create a skill with specific content
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "A test skill for prompt injection".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "You are a specialized test assistant with unique capabilities.".to_string(),
        tags: vec!["test".to_string()],
        version: Some("1.0.0".to_string()),
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill));

    // Verify the skill registry is set
    let session_opts = SessionOptions::new()
        .with_skill_registry(Arc::new(registry));

    assert!(session_opts.skill_registry.is_some());

    let registry_ref = session_opts.skill_registry.as_ref().unwrap();
    assert_eq!(registry_ref.len(), 1);

    // Verify the skill content is available in system prompt
    let system_prompt = registry_ref.to_system_prompt();
    assert!(
        system_prompt.contains("test-skill"),
        "System prompt should contain skill name"
    );
    assert!(
        system_prompt.contains("specialized test assistant"),
        "System prompt should contain skill content"
    );
}

#[tokio::test]
async fn test_multiple_skills_system_prompt() {
    // Create multiple skills
    let skill1 = Skill {
        name: "skill-one".to_string(),
        description: "First skill".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Skill one instructions.".to_string(),
        tags: vec![],
        version: None,
    };

    let skill2 = Skill {
        name: "skill-two".to_string(),
        description: "Second skill".to_string(),
        allowed_tools: Some("grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Skill two instructions.".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill1));
    registry.register(Arc::new(skill2));

    // Generate system prompt
    let system_prompt = registry.to_system_prompt();

    // Verify both skills are included
    assert!(system_prompt.contains("skill-one"));
    assert!(system_prompt.contains("skill-two"));
    assert!(system_prompt.contains("Skill one instructions"));
    assert!(system_prompt.contains("Skill two instructions"));
    assert!(system_prompt.contains("Available Skills"));
}

#[tokio::test]
async fn test_empty_registry_no_injection() {
    let registry = SkillRegistry::new();

    // Empty registry should produce empty system prompt
    let system_prompt = registry.to_system_prompt();
    assert!(system_prompt.is_empty());
}

#[tokio::test]
async fn test_non_instruction_skills_not_injected() {
    // Create a Tool-type skill (not Instruction)
    let tool_skill = Skill {
        name: "tool-skill".to_string(),
        description: "A tool skill".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Tool,  // Not Instruction
        content: "Tool skill content.".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(tool_skill));

    // Tool skills should not be included in system prompt
    let system_prompt = registry.to_system_prompt();
    assert!(system_prompt.is_empty());
}

#[test]
fn test_skill_registry_with_builtin_skills() {
    let registry = SkillRegistry::with_builtins();

    // Should have 7 built-in skills (4 code assistance + 3 tool documentation)
    assert_eq!(registry.len(), 7);

    // Generate system prompt
    let system_prompt = registry.to_system_prompt();

    // Verify code assistance skills are included
    assert!(system_prompt.contains("code-search"));
    assert!(system_prompt.contains("code-review"));
    assert!(system_prompt.contains("explain-code"));
    assert!(system_prompt.contains("find-bugs"));

    // Verify tool documentation skills are included
    assert!(system_prompt.contains("builtin-tools"));
    assert!(system_prompt.contains("delegate-task"));
    assert!(system_prompt.contains("find-skills"));

    // Verify it's properly formatted
    assert!(system_prompt.contains("# Available Skills"));
    assert!(system_prompt.contains("# Skill:"));
}
