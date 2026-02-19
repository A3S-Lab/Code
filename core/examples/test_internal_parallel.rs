//! Query-Lane Tool Parallelization Test
//!
//! Demonstrates A3S Code's Query-lane tool parallelization with slow I/O operations.
//! Parallelization is OPT-IN (default: serial execution). Users control when and how
//! to parallelize via SessionQueueConfig.
//!
//! This test uses web_fetch to demonstrate real performance benefits, as network I/O
//! is significantly slower than local file operations.
//!
//! Run with: cargo run --example test_internal_parallel

use a3s_code_core::{Agent, SessionOptions, SessionQueueConfig};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::queue::ParallelizationStrategy;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn find_config() -> Result<PathBuf> {
    let home_config = dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    if let Some(config) = home_config {
        return Ok(config);
    }

    let project_config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    project_config.ok_or_else(|| anyhow::anyhow!("Config file not found"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=info")
        .init();

    println!("🚀 A3S Code - Query-Lane Tool Parallelization Test\n");
    println!("{}", "=".repeat(80));

    let config_path = find_config()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));
    println!();

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    println!("📌 Test Scenario: Fetch 10 web pages");
    println!("   This demonstrates real performance benefits with slow I/O operations.\n");

    // Test 1: Default behavior (serial execution, no parallelization)
    test_default_serial(&agent).await?;

    // Test 2: Enabled parallelization with custom strategy
    test_enabled_parallel(&agent).await?;

    println!("\n{}", "=".repeat(80));
    println!("✅ All parallelization tests completed!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Test 1: Default behavior - serial execution (parallelization disabled by default)
async fn test_default_serial(agent: &Agent) -> Result<()> {
    println!("\n📦 Test 1: Default Behavior (Serial Execution)");
    println!("{}", "-".repeat(80));
    println!("Task: Fetch 10 web pages with default configuration\n");

    // Default: enable_parallelization = false (serial execution)
    // Use permissive policy so web_fetch doesn't require HITL confirmation
    let mut opts = SessionOptions::default();
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = agent.session(".", Some(opts))?;

    let start = Instant::now();

    // Construct a task that will trigger >= 8 tool calls in a single LLM turn
    let result = session
        .send(
            "Fetch the following web pages and extract their titles:\n\
             1. https://www.rust-lang.org/\n\
             2. https://tokio.rs/\n\
             3. https://docs.rs/\n\
             4. https://crates.io/\n\
             5. https://github.com/rust-lang/rust\n\
             6. https://blog.rust-lang.org/\n\
             7. https://www.rust-lang.org/learn\n\
             8. https://www.rust-lang.org/tools\n\
             9. https://www.rust-lang.org/governance\n\
             10. https://www.rust-lang.org/community\n\
             \n\
             Fetch all pages at once using web_fetch tool, don't do them one by one.",
            None,
        )
        .await?;

    let duration = start.elapsed();

    println!("✓ Completed in: {:.2}s", duration.as_secs_f64());
    println!("✓ Result length: {} chars", result.text.len());
    println!("✓ Tool calls: {}", result.tool_calls_count);
    println!("\n💡 Default: enable_parallelization = false (serial execution)");
    println!("   Expected: ~10 * avg_fetch_time (network latency adds up)\n");

    Ok(())
}

/// Test 2: Enabled parallelization - tools execute in parallel
async fn test_enabled_parallel(agent: &Agent) -> Result<()> {
    println!("\n⚡ Test 2: Enabled Parallelization (Parallel Execution)");
    println!("{}", "-".repeat(80));
    println!("Task: Fetch 10 web pages in parallel via opt-in configuration\n");

    // Create SessionQueueConfig with parallelization ENABLED
    let mut queue_config = SessionQueueConfig::default();
    queue_config.enable_parallelization = true; // OPT-IN: explicitly enable
    queue_config.query_max_concurrency = 10; // Allow 10 concurrent web fetches

    // Custom strategy: lower threshold, only allow web operations
    let mut strategy = ParallelizationStrategy::default();
    strategy.min_tool_count = 3; // Lower threshold: 3 tools trigger parallelization
    strategy.allowed_tools = vec![
        "web_fetch".to_string(),
        "web_search".to_string(),
    ];

    queue_config.parallelization_strategy = Some(strategy);

    println!("✓ SessionQueueConfig created");
    println!("  enable_parallelization: true (OPT-IN)");
    println!("  Query lane max concurrency: 10");
    println!("  Custom strategy:");
    println!("    - min_tool_count: 3 (lower threshold)");
    println!("    - allowed_tools: [web_fetch, web_search]");
    println!("    - blocked_tools: [bash, write, edit, patch]\n");

    // Create session with queue config + permissive policy
    let mut opts = SessionOptions::default().with_queue_config(queue_config);
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = agent.session(".", Some(opts))?;

    let start = Instant::now();

    // Same task as Test 1 - but now with parallelization enabled
    let result = session
        .send(
            "Fetch the following web pages and extract their titles:\n\
             1. https://www.rust-lang.org/\n\
             2. https://tokio.rs/\n\
             3. https://docs.rs/\n\
             4. https://crates.io/\n\
             5. https://github.com/rust-lang/rust\n\
             6. https://blog.rust-lang.org/\n\
             7. https://www.rust-lang.org/learn\n\
             8. https://www.rust-lang.org/tools\n\
             9. https://www.rust-lang.org/governance\n\
             10. https://www.rust-lang.org/community\n\
             \n\
             Fetch all pages at once using web_fetch tool, don't do them one by one.",
            None,
        )
        .await?;

    let duration = start.elapsed();

    println!("\n✓ Completed in: {:.2}s", duration.as_secs_f64());
    println!("✓ Result length: {} chars", result.text.len());
    println!("✓ Tool calls: {}", result.tool_calls_count);
    println!("\n💡 Parallelization enabled: web_fetch calls execute in parallel");
    println!("   Expected: ~max(fetch_times) instead of sum(fetch_times)");
    println!("   Speedup: 3-8x for network I/O operations\n");

    Ok(())
}
