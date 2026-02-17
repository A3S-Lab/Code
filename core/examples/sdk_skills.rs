//! Skills SDK example — built-in skills, catalog injection, and delegate-task.
//!
//! Demonstrates:
//! - Loading built-in skills (`builtin_skills()`)
//! - Verifying delegate-task skill content and structure
//! - Skill catalog injection (`build_skills_injection()`)
//! - TaskTool / ParallelTaskTool descriptions include agent info
//!
//! No LLM calls — runs entirely offline.
//!
//! ## Usage
//!
//! ```bash
//! cd crates/code && cargo run --example sdk_skills
//! ```

use a3s_code_core::tools::{
    build_skills_injection, builtin_skills, Skill, SkillKind, DEFAULT_CATALOG_THRESHOLD,
};

fn main() {
    println!("=== A3S Code SDK Skills Example ===\n");

    // ── 1. Load built-in skills ──────────────────────────────────────────
    println!("--- Built-in Skills ---");
    let skills = builtin_skills();
    println!("  count: {}", skills.len());
    assert!(
        skills.len() >= 2,
        "expected at least 2 built-in skills, got {}",
        skills.len()
    );

    for skill in &skills {
        println!(
            "  - {} (kind={:?}, desc_len={}, content_len={})",
            skill.name,
            skill.kind,
            skill.description.len(),
            skill.content.len()
        );
    }
    println!();

    // ── 2. Verify find-skills ────────────────────────────────────────────
    println!("--- find-skills ---");
    let find = skills.iter().find(|s| s.name == "find-skills");
    assert!(find.is_some(), "find-skills should be present");
    let find = find.unwrap();
    assert_eq!(find.kind, SkillKind::Instruction);
    assert!(find.content.contains("search_skills"));
    assert!(find.content.contains("install_skill"));
    println!("  [ok] name={}", find.name);
    println!("  [ok] kind=Instruction");
    println!("  [ok] content references search_skills, install_skill");
    println!();

    // ── 3. Verify delegate-task ──────────────────────────────────────────
    println!("--- delegate-task ---");
    let delegate = skills.iter().find(|s| s.name == "delegate-task");
    assert!(delegate.is_some(), "delegate-task should be present");
    let delegate = delegate.unwrap();

    // Kind
    assert_eq!(delegate.kind, SkillKind::Instruction);
    println!("  [ok] kind=Instruction");

    // Description
    assert!(
        delegate.description.contains("sub-agent"),
        "description should mention sub-agents"
    );
    println!(
        "  [ok] description: {}",
        &delegate.description[..delegate.description.len().min(80)]
    );

    // Content: built-in agents
    assert!(delegate.content.contains("explore"), "should reference explore");
    assert!(delegate.content.contains("general"), "should reference general");
    assert!(delegate.content.contains("plan"), "should reference plan");
    println!("  [ok] content references agents: explore, general, plan");

    // Content: custom agents
    assert!(
        delegate.content.contains("agent_dirs"),
        "should mention agent_dirs"
    );
    println!("  [ok] content mentions agent_dirs for custom agents");

    // Content: parallel_task
    assert!(
        delegate.content.contains("parallel_task"),
        "should reference parallel_task"
    );
    println!("  [ok] content references parallel_task tool");

    // Content: best practices
    assert!(
        delegate.content.contains("Best Practices"),
        "should have best practices section"
    );
    println!("  [ok] content includes Best Practices section");
    println!();

    // ── 4. Skill catalog injection (full mode) ───────────────────────────
    println!("--- Skill Catalog Injection (full mode) ---");
    let full_injection = build_skills_injection(&skills, DEFAULT_CATALOG_THRESHOLD);
    assert!(
        !full_injection.is_empty(),
        "injection should not be empty with {} skills",
        skills.len()
    );

    // 2 skills <= threshold 3 → full mode
    assert!(
        full_injection.contains("<skills>"),
        "should use full mode (<skills> tag)"
    );
    assert!(
        full_injection.contains("delegate-task"),
        "injection should include delegate-task"
    );
    assert!(
        full_injection.contains("find-skills"),
        "injection should include find-skills"
    );
    println!("  threshold: {}", DEFAULT_CATALOG_THRESHOLD);
    println!("  instruction skills: {}", skills.len());
    println!("  [ok] full mode: <skills> tag present");
    println!("  [ok] delegate-task included in injection");
    println!("  [ok] find-skills included in injection");
    println!();

    // ── 5. Skill catalog injection (catalog mode) ────────────────────────
    println!("--- Skill Catalog Injection (catalog mode, threshold=1) ---");
    let catalog_injection = build_skills_injection(&skills, 1);
    assert!(
        catalog_injection.contains("<skill-catalog>"),
        "should use catalog mode with threshold=1"
    );
    assert!(
        catalog_injection.contains("delegate-task"),
        "catalog should list delegate-task"
    );
    assert!(
        catalog_injection.contains("load_skill"),
        "catalog should reference load_skill"
    );
    println!("  threshold: 1");
    println!("  [ok] catalog mode: <skill-catalog> tag present");
    println!("  [ok] delegate-task listed in catalog");
    println!("  [ok] catalog references load_skill for on-demand loading");
    println!();

    // ── 6. Custom skills alongside builtins ──────────────────────────────
    println!("--- Custom Skills + Builtins ---");
    let mut combined = skills.clone();
    combined.push(Skill {
        name: "custom-deploy".to_string(),
        description: "Deploy to production".to_string(),
        allowed_tools: Some("Bash(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Custom deployment instructions.".to_string(),
    });
    combined.push(Skill {
        name: "custom-review".to_string(),
        description: "Code review assistant".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Custom code review instructions.".to_string(),
    });

    let combined_injection = build_skills_injection(&combined, DEFAULT_CATALOG_THRESHOLD);

    // 4 skills > threshold 3 → catalog mode
    assert!(
        combined_injection.contains("<skill-catalog>"),
        "4 skills should trigger catalog mode"
    );
    assert!(combined_injection.contains("delegate-task"));
    assert!(combined_injection.contains("find-skills"));
    assert!(combined_injection.contains("custom-deploy"));
    assert!(combined_injection.contains("custom-review"));
    println!("  total instruction skills: {}", combined.len());
    println!("  [ok] catalog mode triggered (count > threshold)");
    println!("  [ok] all 4 skills listed in catalog");
    println!();

    // ── 7. Tool-kind skills don't affect injection ───────────────────────
    println!("--- Tool-kind Skills Excluded ---");
    let mut with_tool = skills.clone();
    with_tool.push(Skill {
        name: "my-tool".to_string(),
        description: "A tool skill".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Tool,
        content: "Tool content".to_string(),
    });

    let tool_injection = build_skills_injection(&with_tool, DEFAULT_CATALOG_THRESHOLD);
    // 2 instruction + 1 tool → still 2 instruction → full mode
    assert!(
        tool_injection.contains("<skills>"),
        "tool-kind should not count toward threshold"
    );
    assert!(
        !tool_injection.contains("my-tool"),
        "tool-kind should not appear in injection"
    );
    println!("  [ok] Tool-kind skills excluded from injection");
    println!("  [ok] Tool-kind does not affect threshold count");
    println!();

    // ── Done ─────────────────────────────────────────────────────────────
    println!("=== All checks passed ===");
}
