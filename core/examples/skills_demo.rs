//! Skills System Demo
//!
//! Demonstrates the Claude Code-compatible skill system.

use a3s_code_core::{
    skills::{builtin_skills, Skill, SkillRegistry},
    SessionOptions,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("=== A3S Code Skills System Demo ===\n");

    // 1. Using built-in skills
    println!("1. Built-in Skills");
    println!("------------------");
    let builtins = builtin_skills();
    println!("Available built-in skills: {}", builtins.len());
    for skill in &builtins {
        println!("  - {}: {}", skill.name, skill.description);
    }
    println!();

    // 2. Create a skill registry with built-ins
    println!("2. Skill Registry");
    println!("-----------------");
    let registry = SkillRegistry::with_builtins();
    println!("Registry contains {} skills", registry.len());
    println!("Skill names: {:?}", registry.list());
    println!();

    // 3. Get a specific skill
    println!("3. Skill Details");
    println!("----------------");
    if let Some(skill) = registry.get("code-search") {
        println!("Name: {}", skill.name);
        println!("Description: {}", skill.description);
        println!("Kind: {:?}", skill.kind);
        println!("Tags: {:?}", skill.tags);
        println!("Allowed tools: {:?}", skill.allowed_tools);
        println!();

        // Check tool permissions
        println!("Tool permissions:");
        println!("  - grep allowed: {}", skill.is_tool_allowed("grep"));
        println!("  - read allowed: {}", skill.is_tool_allowed("read"));
        println!("  - write allowed: {}", skill.is_tool_allowed("write"));
        println!();
    }

    // 4. Filter skills by kind
    println!("4. Filter by Kind");
    println!("-----------------");
    let instruction_skills = registry.by_kind(a3s_code_core::skills::SkillKind::Instruction);
    println!("Instruction skills: {}", instruction_skills.len());
    for skill in &instruction_skills {
        println!("  - {}", skill.name);
    }
    println!();

    // 5. Filter skills by tag
    println!("5. Filter by Tag");
    println!("----------------");
    let search_skills = registry.by_tag("search");
    println!("Skills tagged 'search': {}", search_skills.len());
    for skill in &search_skills {
        println!("  - {}", skill.name);
    }
    println!();

    let security_skills = registry.by_tag("security");
    println!("Skills tagged 'security': {}", security_skills.len());
    for skill in &security_skills {
        println!("  - {}", skill.name);
    }
    println!();

    // 6. Generate system prompt
    println!("6. System Prompt Generation");
    println!("---------------------------");
    let system_prompt = registry.to_system_prompt();
    println!("System prompt length: {} characters", system_prompt.len());
    println!("Preview (first 200 chars):");
    println!("{}", &system_prompt[..200.min(system_prompt.len())]);
    println!("...\n");

    // 7. Create a custom skill
    println!("7. Custom Skill");
    println!("---------------");
    let custom_skill = Skill {
        name: "custom-analyzer".to_string(),
        description: "Analyze code for custom patterns".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: a3s_code_core::skills::SkillKind::Instruction,
        content: r#"# Custom Analyzer

You are a custom code analyzer. Look for specific patterns in the codebase.

## Instructions

1. Use grep to search for patterns
2. Read files to verify matches
3. Report findings with file paths and line numbers
"#
        .to_string(),
        tags: vec!["analysis".to_string(), "custom".to_string()],
        version: Some("1.0.0".to_string()),
    };

    registry.register(Arc::new(custom_skill)).unwrap();
    println!("Registered custom skill: custom-analyzer");
    println!("Total skills now: {}", registry.len());
    println!();

    // 8. Load skills from directory (if exists)
    println!("8. Load from Directory");
    println!("----------------------");
    let skills_dir = "core/skills";
    match registry.load_from_dir(skills_dir) {
        Ok(loaded) => {
            println!("Loaded {} skills from {}", loaded, skills_dir);
            println!("Total skills now: {}", registry.len());
        }
        Err(e) => {
            println!("Could not load from {}: {}", skills_dir, e);
        }
    }
    println!();

    // 9. Integration with SessionOptions
    println!("9. SessionOptions Integration");
    println!("------------------------------");

    // Method 1: Use built-in skills
    let _options1 = SessionOptions::new().with_builtin_skills();
    println!("✓ Created SessionOptions with built-in skills");

    // Method 2: Use custom registry
    let custom_registry = Arc::new(SkillRegistry::with_builtins());
    let _options2 = SessionOptions::new().with_skill_registry(custom_registry);
    println!("✓ Created SessionOptions with custom registry");

    // Method 3: Load from directory
    let _options3 = SessionOptions::new().with_skills_from_dir("core/skills");
    println!("✓ Created SessionOptions with skills from directory");
    println!();

    // 10. Parse skill from markdown
    println!("10. Parse Skill from Markdown");
    println!("------------------------------");
    let markdown = r#"---
name: test-skill
description: A test skill for demonstration
allowed-tools: "read(*), grep(*)"
kind: instruction
tags:
  - test
  - demo
version: 1.0.0
---
# Test Skill

This is a test skill to demonstrate the Claude Code skill format.

## Features

- Markdown with YAML frontmatter
- Tool permissions
- Tags and versioning
"#;

    if let Some(parsed_skill) = Skill::parse(markdown) {
        println!("✓ Successfully parsed skill from markdown");
        println!("  Name: {}", parsed_skill.name);
        println!("  Description: {}", parsed_skill.description);
        println!("  Tags: {:?}", parsed_skill.tags);
        println!("  Version: {:?}", parsed_skill.version);
    } else {
        println!("✗ Failed to parse skill");
    }
    println!();

    println!("=== Demo Complete ===");
    println!("\nThe skill system is fully compatible with Claude Code format!");
    println!("You can now:");
    println!("  1. Use built-in skills (code-search, code-review, explain-code, find-bugs)");
    println!("  2. Create custom skills in Markdown format");
    println!("  3. Load skills from directories");
    println!("  4. Filter skills by kind or tag");
    println!("  5. Integrate with SessionOptions");

    Ok(())
}
