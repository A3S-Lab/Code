//! Auto-Compact Example
//!
//! Demonstrates context window auto-compaction: when token usage exceeds
//! the configured threshold, old messages are summarized automatically.
//!
//! Run with: cargo run --example test_auto_compact

use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;

fn find_config() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .expect("Config not found. Create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("A3S Code — Auto-Compact Example");
    println!("{}", "=".repeat(50));

    let agent = Agent::new(find_config().to_str().unwrap()).await?;

    // Enable auto-compact at 80% context usage
    let opts = SessionOptions::new()
        .with_permissive_policy()
        .with_auto_compact(true)
        .with_auto_compact_threshold(0.80);

    let session = agent.session(std::env::temp_dir().to_str().unwrap(), Some(opts))?;

    println!("✓ Session created with auto-compact enabled (threshold: 80%)");

    // Send a few messages to build up context
    let result = session.send("What is Rust's ownership model? Explain briefly.", None).await?;
    println!("Turn 1: {} tokens used", result.usage.total_tokens);

    let result = session.send("What are lifetimes in Rust?", None).await?;
    println!("Turn 2: {} tokens used", result.usage.total_tokens);

    let result = session.send("Summarize what we discussed.", None).await?;
    println!("Turn 3: {} tokens used", result.usage.total_tokens);
    println!("Response: {}", &result.text[..result.text.len().min(120)]);

    println!("\n✓ Auto-compact example complete");
    Ok(())
}
