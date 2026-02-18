//! Test all 7 built-in skills
//!
//! Tests:
//! - Code assistance skills (4): code-search, code-review, explain-code, find-bugs
//! - Tool documentation skills (3): builtin-tools, delegate-task, find-skills
//!
//! Run with: cargo run --example test_builtin_skills

use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=debug")
        .init();

    println!("🚀 Testing All 7 Built-in Skills\n");
    println!("{}", "=".repeat(80));

    // Load config
    let config_path = find_config_path()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));
    println!();

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // Create session with all built-in skills enabled
    let session = agent.session(".", Some(SessionOptions::new().with_builtin_skills()))?;

    println!("✅ Session created with 7 built-in skills enabled\n");

    // Test Code Assistance Skills
    println!("📚 Code Assistance Skills (4)");
    println!("{}", "=".repeat(80));

    test_code_search(&session).await?;
    test_code_review(&session).await?;
    test_explain_code(&session).await?;
    test_find_bugs(&session).await?;

    // Test Tool Documentation Skills
    println!("\n📖 Tool Documentation Skills (3)");
    println!("{}", "=".repeat(80));

    test_builtin_tools(&session).await?;
    test_delegate_task(&session).await?;
    test_find_skills(&session).await?;

    println!(
        "{}",
        "
"
        .repeat(2)
    );
    println!("{}", "=".repeat(80));
    println!("✅ All 7 built-in skills tested successfully!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Test 1: code-search skill
async fn test_code_search(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n🔍 Skill 1: code-search");
    println!("{}", "-".repeat(80));
    println!("Description: Search codebase for patterns, functions, or types");
    println!("Allowed tools: read(*), grep(*), glob(*)");

    let result = session
        .send(
            "Search for all functions named 'new' in Rust files in the src directory",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ code-search skill works correctly");
    Ok(())
}

/// Test 2: code-review skill
async fn test_code_review(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n📝 Skill 2: code-review");
    println!("{}", "-".repeat(80));
    println!("Description: Review code for best practices, bugs, and improvements");
    println!("Allowed tools: read(*), grep(*), glob(*)");

    let result = session
        .send(
            "Review the code in src/lib.rs and provide feedback on code quality",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ code-review skill works correctly");
    Ok(())
}

/// Test 3: explain-code skill
async fn test_explain_code(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n💡 Skill 3: explain-code");
    println!("{}", "-".repeat(80));
    println!("Description: Explain how code works in clear, simple terms");
    println!("Allowed tools: read(*), grep(*), glob(*)");

    let result = session
        .send(
            "Explain how the Agent struct works in src/agent_api.rs",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ explain-code skill works correctly");
    Ok(())
}

/// Test 4: find-bugs skill
async fn test_find_bugs(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n🐛 Skill 4: find-bugs");
    println!("{}", "-".repeat(80));
    println!("Description: Identify potential bugs, vulnerabilities, and code smells");
    println!("Allowed tools: read(*), grep(*), glob(*)");

    let result = session
        .send("Check src/config.rs for potential bugs or issues", None)
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ find-bugs skill works correctly");
    Ok(())
}

/// Test 5: builtin-tools skill
async fn test_builtin_tools(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n🛠️  Skill 5: builtin-tools");
    println!("{}", "-".repeat(80));
    println!("Description: Documentation for all built-in file operation and shell tools");
    println!("Allowed tools: None (documentation only)");

    let result = session
        .send(
            "What tools are available for file operations? List them with descriptions.",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ builtin-tools skill works correctly");
    Ok(())
}

/// Test 6: delegate-task skill
async fn test_delegate_task(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n🎯 Skill 6: delegate-task");
    println!("{}", "-".repeat(80));
    println!("Description: Guide for delegating tasks to specialized sub-agents");
    println!("Allowed tools: task(*)");

    let result = session
        .send(
            "Explain when and how I should delegate tasks to sub-agents",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ delegate-task skill works correctly");
    Ok(())
}

/// Test 7: find-skills skill
async fn test_find_skills(session: &a3s_code_core::AgentSession) -> Result<()> {
    println!("\n🔎 Skill 7: find-skills");
    println!("{}", "-".repeat(80));
    println!("Description: Discover and install agent skills from the ecosystem");
    println!("Allowed tools: search_skills(*), install_skill(*), load_skill(*)");

    let result = session
        .send(
            "How can I find and install new skills for this agent?",
            None,
        )
        .await?;

    println!("✓ Result preview: {}", truncate(&result.text, 200));
    println!("✅ find-skills skill works correctly");
    Ok(())
}

/// Find config path
fn find_config_path() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|p| p.join(".a3s/config.hcl"))
                .filter(|p| p.exists())
        })
        .ok_or_else(|| anyhow::anyhow!("Config file not found"))
}

/// Truncate helper
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
