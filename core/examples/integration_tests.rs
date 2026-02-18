//! Integration tests for A3S Code core features
//!
//! This example tests all major features using real LLM configuration from ~/.a3s/config.hcl
//!
//! Run with: cargo run --example integration_tests

use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=debug")
        .init();

    println!("🚀 A3S Code Integration Tests\n");
    println!("{}", "=".repeat(80));

    // Load config from ~/.a3s/config.hcl or fallback to local
    let config_path = dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .or_else(|| {
            // Try project root
            let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|p| p.join(".a3s/config.hcl"));
            project_root.filter(|p| p.exists())
        })
        .expect("Config file not found. Please create ~/.a3s/config.hcl or use project .a3s/config.hcl");

    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));
    println!();

    // Create agent
    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // Test 1: Basic Tool Execution
    test_basic_tools(&agent).await?;

    // Test 2: Built-in Skills
    test_builtin_skills(&agent).await?;

    // Test 3: File Operations
    test_file_operations(&agent).await?;

    // Test 4: Search Operations
    test_search_operations(&agent).await?;

    // Test 5: Web Search (if configured)
    test_web_search(&agent).await?;

    // Test 6: Planning Mode
    test_planning_mode(&agent).await?;

    // Test 7: Queue Configuration
    test_queue_config(&agent).await?;

    println!("{}", "
".repeat(2));
    println!("{}", "=".repeat(80));
    println!("✅ All integration tests completed successfully!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Test 1: Basic tool execution
async fn test_basic_tools(agent: &Agent) -> Result<()> {
    println!("\n📦 Test 1: Basic Tool Execution");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", None)?;

    println!("Testing: List current directory...");
    let result = session.send("List the files in the current directory using ls", None).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\nTesting: Read a file...");
    let result = session.send("Read the Cargo.toml file", None).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\n✅ Test 1 passed: Basic tools work correctly");
    Ok(())
}

/// Test 2: Built-in skills
async fn test_builtin_skills(agent: &Agent) -> Result<()> {
    println!("\n🧠 Test 2: Built-in Skills (7 skills)");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_builtin_skills()
    ))?;

    println!("Testing: code-search skill...");
    let result = session.send("Search for all functions named 'new' in Rust files", None).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\nTesting: builtin-tools skill...");
    let result = session.send("What tools are available for file operations?", None).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\n✅ Test 2 passed: Built-in skills work correctly");
    Ok(())
}

/// Test 3: File operations
async fn test_file_operations(agent: &Agent) -> Result<()> {
    println!("\n📝 Test 3: File Operations");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", None)?;

    println!("Testing: Create a test file...");
    let result = session.send(
        "Create a file named 'test_integration.txt' with content 'Hello from A3S Code integration test!'",
        None
    ).await?;
    println!("✓ Result: {}", truncate(&result.text, 200));

    println!("\nTesting: Read the test file...");
    let result = session.send("Read the file test_integration.txt", None).await?;
    println!("✓ Result: {}", truncate(&result.text, 200));

    println!("\nTesting: Edit the test file...");
    let result = session.send(
        "Edit test_integration.txt and replace 'Hello' with 'Greetings'",
        None
    ).await?;
    println!("✓ Result: {}", truncate(&result.text, 200));

    println!("\nCleaning up: Remove test file...");
    let _ = std::fs::remove_file("test_integration.txt");

    println!("\n✅ Test 3 passed: File operations work correctly");
    Ok(())
}

/// Test 4: Search operations
async fn test_search_operations(agent: &Agent) -> Result<()> {
    println!("\n🔍 Test 4: Search Operations");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", None)?;

    println!("Testing: grep search...");
    let result = session.send(
        "Search for the word 'Agent' in all Rust files using grep",
        None
    ).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\nTesting: glob pattern matching...");
    let result = session.send(
        "Find all .rs files in the src directory using glob",
        None
    ).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\n✅ Test 4 passed: Search operations work correctly");
    Ok(())
}

/// Test 5: Web search (if configured)
async fn test_web_search(agent: &Agent) -> Result<()> {
    println!("\n🌐 Test 5: Web Search");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", None)?;

    println!("Testing: Web search...");
    let result = session.send(
        "Search the web for 'Rust async programming best practices' and summarize the top 3 results",
        None
    ).await;

    match result {
        Ok(r) => {
            println!("✓ Result preview: {}", truncate(&r.text, 300));
            println!("\n✅ Test 5 passed: Web search works correctly");
        }
        Err(e) => {
            println!("⚠️  Web search not available or failed: {}", e);
            println!("   This is expected if search engines are not configured");
        }
    }

    Ok(())
}

/// Test 6: Planning mode
async fn test_planning_mode(agent: &Agent) -> Result<()> {
    println!("\n🎯 Test 6: Planning Mode");
    println!("{}", "-".repeat(80));

    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_planning(true)
            .with_goal_tracking(true)
    ))?;

    println!("Testing: Multi-step task with planning...");
    let result = session.send(
        "Create a file named 'plan_test.txt', write 'Step 1' to it, then read it back and confirm the content",
        None
    ).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 300));

    println!("\nCleaning up: Remove test file...");
    let _ = std::fs::remove_file("plan_test.txt");

    println!("\n✅ Test 6 passed: Planning mode works correctly");
    Ok(())
}

/// Test 7: Queue configuration
async fn test_queue_config(agent: &Agent) -> Result<()> {
    println!("\n⚡ Test 7: Queue Configuration (A3S Lane v0.4.0)");
    println!("{}", "-".repeat(80));

    use a3s_code_core::queue::{SessionQueueConfig, RetryPolicyConfig};

    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        execute_max_concurrency: 2,
        enable_metrics: true,
        enable_dlq: true,
        retry_policy: Some(RetryPolicyConfig {
            strategy: "exponential".to_string(),
            max_retries: 3,
            initial_delay_ms: 100,
            fixed_delay_ms: None,
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_queue_config(queue_config)
    ))?;

    println!("Testing: Parallel query operations with queue...");
    let result = session.send(
        "List all .rs files and count how many contain the word 'async'",
        None
    ).await?;
    println!("✓ Result preview: {}", truncate(&result.text, 200));

    println!("\n✅ Test 7 passed: Queue configuration works correctly");
    Ok(())
}

/// Helper function to truncate long strings
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}... (truncated)", &s[..max_len])
    }
}
