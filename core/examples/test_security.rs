//! Security Provider Example
//!
//! Demonstrates the DefaultSecurityProvider: input taint tracking and
//! output sanitization for sensitive data patterns.
//!
//! Run with: cargo run --example test_security

use a3s_code_core::security::DefaultSecurityProvider;
use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

fn find_config() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .expect("Config not found. Create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("A3S Code — Security Provider Example");
    println!("{}", "=".repeat(50));

    let agent = Agent::new(find_config().to_str().unwrap()).await?;

    // Test 1: Default security provider
    println!("\n--- Test 1: Default Security Provider ---");
    let opts = SessionOptions::new()
        .with_permissive_policy()
        .with_default_security();

    let session = agent.session(std::env::temp_dir().to_str().unwrap(), Some(opts))?;
    let result = session.send("What is 2 + 2?", None).await?;
    println!("✓ Response: {}", result.text.trim());

    // Test 2: Custom security provider
    println!("\n--- Test 2: Custom Security Provider ---");
    let provider = Arc::new(DefaultSecurityProvider::new());
    let opts = SessionOptions::new()
        .with_permissive_policy()
        .with_security_provider(provider);

    let session = agent.session(std::env::temp_dir().to_str().unwrap(), Some(opts))?;
    let result = session.send("Say hello.", None).await?;
    println!("✓ Response: {}", result.text.trim());

    println!("\n✓ Security example complete");
    Ok(())
}
