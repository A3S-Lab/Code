//! Test A3S Code with local .a3s/config.hcl
//!
//! This example demonstrates using the A3S Code framework with the
//! configuration file located at D:\CODE\a3s\.a3s\config.hcl
//!
//! Run with: cargo run --example test_a3s_config

use a3s_code_core::{Agent, SessionOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("=== A3S Code Configuration Test ===\n");

    // Load configuration from root .a3s/config.hcl
    let config_path = "D:\\CODE\\a3s\\.a3s\\config.hcl";
    println!("Loading configuration from: {}", config_path);

    let agent = Agent::new(config_path).await?;
    println!("✓ Agent initialized successfully\n");

    // Create a session with permissive policy for testing
    let opts = SessionOptions::new()
        .with_permissive_policy()
        .with_builtin_skills();

    let workspace = "D:\\CODE\\a3s";
    println!("Creating session with workspace: {}", workspace);
    let session = agent.session(workspace, Some(opts))?;
    println!("✓ Session created successfully\n");

    // Test 1: Simple query
    println!("--- Test 1: Simple Query ---");
    let result = session
        .send("What is 2 + 2? Just answer with the number.", None)
        .await?;
    println!("Response: {}", result.text);
    println!("Tokens used: {}", result.usage.total_tokens);
    println!();

    // Test 2: Code-related query
    println!("--- Test 2: Code Query ---");
    let result = session
        .send(
            "List the top-level directories in the current workspace.",
            None,
        )
        .await?;
    println!("Response: {}", result.text);
    println!("Tokens used: {}", result.usage.total_tokens);
    println!();

    println!("=== All Tests Completed Successfully ===");
    Ok(())
}
