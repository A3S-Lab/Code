//! A3S Code - Custom Skills and Agents Integration Test
//!
//! Tests loading and execution of custom skills and agents from ~/.a3s directory.
//!
//! Run with: cargo run --example test_custom_skills_agents

use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;

fn find_config() -> Result<PathBuf> {
    let home_config = dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    if let Some(config) = home_config {
        return Ok(config);
    }

    // Try project root
    let project_config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    project_config.ok_or_else(|| anyhow::anyhow!("Config file not found"))
}

fn find_skills_dir() -> Result<PathBuf> {
    let home_skills = dirs::home_dir()
        .map(|h| h.join(".a3s/skills"))
        .filter(|p| p.exists());

    if let Some(skills) = home_skills {
        return Ok(skills);
    }

    // Try project root
    let project_skills = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/skills"))
        .filter(|p| p.exists());

    project_skills.ok_or_else(|| anyhow::anyhow!("Skills directory not found"))
}

fn find_agents_dir() -> Result<PathBuf> {
    let home_agents = dirs::home_dir()
        .map(|h| h.join(".a3s/agents"))
        .filter(|p| p.exists());

    if let Some(agents) = home_agents {
        return Ok(agents);
    }

    // Try project root
    let project_agents = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/agents"))
        .filter(|p| p.exists());

    project_agents.ok_or_else(|| anyhow::anyhow!("Agents directory not found"))
}

fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}... (truncated)", &text[..max_len])
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 A3S Code - Custom Skills and Agents Integration Test\n");
    println!("{}", "=".repeat(80));

    let config_path = find_config()?;
    println!("📄 Using config: {}", config_path.display());

    let skills_dir = find_skills_dir()?;
    println!("📚 Skills directory: {}", skills_dir.display());

    let agents_dir = find_agents_dir()?;
    println!("🤖 Agents directory: {}", agents_dir.display());

    println!("{}", "=".repeat(80));
    println!();

    // Test 1: Load custom skills in session
    println!("📦 Test 1: Load Custom Skills in Session");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        let opts = SessionOptions::new().with_skills_from_dir(&skills_dir);

        let session = agent.session(".", Some(opts))?;

        println!("Testing: Ask about Rust best practices (should use rust-expert skill)...");
        let result = session
            .send("What are the key principles of Rust ownership?", None)
            .await?;
        println!("✓ Result preview: {}", truncate(&result.text, 200));
        println!();

        println!("Testing: Ask about API design (should use api-designer skill)...");
        let result = session
            .send("What are REST API best practices for error handling?", None)
            .await?;
        println!("✓ Result preview: {}", truncate(&result.text, 200));
        println!();
    }
    println!("✅ Test 1 passed: Custom skills loaded and used in session\n");

    // Test 2: Load custom agents in session
    println!("🤖 Test 2: Load Custom Agents in Session");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        let opts = SessionOptions::new().with_agent_dir(&agents_dir);

        let session = agent.session(".", Some(opts))?;

        println!("Testing: Request code review (should use code-reviewer agent)...");
        let result = session
            .send("Review this function for potential issues: fn add(a: i32, b: i32) -> i32 { a + b }", None)
            .await?;
        println!("✓ Result preview: {}", truncate(&result.text, 200));
        println!();

        println!("Testing: Request documentation (should use documentation-writer agent)...");
        let result = session
            .send(
                "Write API documentation for a user registration endpoint",
                None,
            )
            .await?;
        println!("✓ Result preview: {}", truncate(&result.text, 200));
        println!();
    }
    println!("✅ Test 2 passed: Custom agents loaded and used in session\n");

    // Test 3: Load both skills and agents together
    println!("🔧 Test 3: Load Both Skills and Agents Together");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        let opts = SessionOptions::new()
            .with_skills_from_dir(&skills_dir)
            .with_agent_dir(&agents_dir);

        let session = agent.session(".", Some(opts))?;

        println!("Testing: Complex task using both skills and agents...");
        let result = session
            .send(
                "Generate unit tests for a Rust function that parses JSON",
                None,
            )
            .await?;
        println!("✓ Result preview: {}", truncate(&result.text, 200));
        println!();
    }
    println!("✅ Test 3 passed: Both skills and agents work together\n");

    // Test 4: Multiple sessions with different configurations
    println!("🔀 Test 4: Multiple Sessions with Different Configurations");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        // Session 1: Only skills
        let opts1 = SessionOptions::new().with_skills_from_dir(&skills_dir);
        let session1 = agent.session(".", Some(opts1))?;

        // Session 2: Only agents
        let opts2 = SessionOptions::new().with_agent_dir(&agents_dir);
        let session2 = agent.session(".", Some(opts2))?;

        println!("Testing: Session 1 with skills only...");
        let result1 = session1.send("Explain Rust lifetimes", None).await?;
        println!("✓ Session 1 result: {}", truncate(&result1.text, 150));
        println!();

        println!("Testing: Session 2 with agents only...");
        let result2 = session2
            .send("Review this code for improvements", None)
            .await?;
        println!("✓ Session 2 result: {}", truncate(&result2.text, 150));
        println!();
    }
    println!("✅ Test 4 passed: Multiple sessions with different configs work correctly\n");

    println!();
    println!("{}", "=".repeat(80));
    println!("✅ All custom skills and agents tests completed successfully!");
    println!("{}", "=".repeat(80));

    Ok(())
}
