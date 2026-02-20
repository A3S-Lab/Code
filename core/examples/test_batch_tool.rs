//! Batch Tool Example
//!
//! Demonstrates the batch tool: executing multiple independent tool calls
//! in parallel within a single LLM turn.
//!
//! Run with: cargo run --example test_batch_tool

use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;
use tempfile::TempDir;

fn find_config() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .expect("Config not found. Create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("A3S Code — Batch Tool Example");
    println!("{}", "=".repeat(50));

    let agent = Agent::new(find_config().to_str().unwrap()).await?;
    let dir = TempDir::new()?;

    // Create some test files
    std::fs::write(dir.path().join("a.txt"), "File A content")?;
    std::fs::write(dir.path().join("b.txt"), "File B content")?;
    std::fs::write(dir.path().join("c.txt"), "File C content")?;

    let opts = SessionOptions::new().with_permissive_policy();
    let session = agent.session(dir.path().to_str().unwrap(), Some(opts))?;

    println!("Workspace: {}", dir.path().display());
    println!("Files: a.txt, b.txt, c.txt");
    println!("\nAsking agent to read all files in parallel using batch tool...\n");

    let result = session
        .send(
            "Use the batch tool to read a.txt, b.txt, and c.txt all at once in parallel. \
         Then summarize what each file contains.",
            None,
        )
        .await?;

    println!("Tool calls: {}", result.tool_calls_count);
    println!("Response:\n{}", result.text);

    println!("\n✓ Batch tool example complete");
    Ok(())
}
