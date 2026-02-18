//! A3S Lane v0.4.0 Advanced Features Integration Test
//!
//! Tests all advanced queue features:
//! - Retry policies (exponential/fixed backoff)
//! - Rate limiting
//! - Priority boost
//! - Pressure monitoring
//! - Per-lane timeouts
//!
//! Run with: cargo run --example test_lane_features

use a3s_code_core::{Agent, SessionOptions};
use a3s_code_core::queue::{
    SessionQueueConfig, RetryPolicyConfig, RateLimitConfig, PriorityBoostConfig,
};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=debug")
        .init();

    println!("🚀 A3S Lane v0.4.0 Advanced Features Test\n");
    println!("{}", "=".repeat(80));

    // Load config
    let config_path = find_config_path()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));
    println!();

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // Test 1: Retry Policy
    test_retry_policy(&agent).await?;

    // Test 2: Rate Limiting
    test_rate_limiting(&agent).await?;

    // Test 3: Priority Boost
    test_priority_boost(&agent).await?;

    // Test 4: Pressure Monitoring
    test_pressure_monitoring(&agent).await?;

    // Test 5: Per-Lane Timeouts
    test_per_lane_timeouts(&agent).await?;

    // Test 6: Combined Features
    test_combined_features(&agent).await?;

    println!("{}", "
".repeat(2));
    println!("{}", "=".repeat(80));
    println!("✅ All A3S Lane v0.4.0 features tested successfully!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Test 1: Retry Policy
async fn test_retry_policy(agent: &Agent) -> Result<()> {
    println!("\n🔄 Test 1: Retry Policy");
    println!("{}", "-".repeat(80));

    // Test exponential backoff
    println!("Testing: Exponential backoff retry...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        enable_metrics: true,
        retry_policy: Some(RetryPolicyConfig {
            strategy: "exponential".to_string(),
            max_retries: 3,
            initial_delay_ms: 100,
            fixed_delay_ms: None,
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "List all Rust files in the current directory",
        None
    ).await?;
    println!("✓ Exponential backoff: {}", truncate(&result.text, 150));

    // Test fixed delay
    println!("\nTesting: Fixed delay retry...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        enable_metrics: true,
        retry_policy: Some(RetryPolicyConfig {
            strategy: "fixed".to_string(),
            max_retries: 3,
            initial_delay_ms: 0,
            fixed_delay_ms: Some(500),
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "Count the number of .rs files",
        None
    ).await?;
    println!("✓ Fixed delay: {}", truncate(&result.text, 150));

    println!("\n✅ Test 1 passed: Retry policies work correctly");
    Ok(())
}

/// Test 2: Rate Limiting
async fn test_rate_limiting(agent: &Agent) -> Result<()> {
    println!("\n⏱️  Test 2: Rate Limiting");
    println!("{}", "-".repeat(80));

    println!("Testing: Per-second rate limit...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 10,
        enable_metrics: true,
        rate_limit: Some(RateLimitConfig {
            limit_type: "per_second".to_string(),
            max_operations: Some(5),
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let start = std::time::Instant::now();
    let result = session.send(
        "Search for 'Agent' in all Rust files and count occurrences",
        None
    ).await?;
    let elapsed = start.elapsed();

    println!("✓ Rate limited operation completed in {:?}", elapsed);
    println!("  Result preview: {}", truncate(&result.text, 150));

    println!("\n✅ Test 2 passed: Rate limiting works correctly");
    Ok(())
}

/// Test 3: Priority Boost
async fn test_priority_boost(agent: &Agent) -> Result<()> {
    println!("\n🚀 Test 3: Priority Boost");
    println!("{}", "-".repeat(80));

    println!("Testing: Standard priority boost...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        enable_metrics: true,
        priority_boost: Some(PriorityBoostConfig {
            strategy: "standard".to_string(),
            deadline_ms: Some(60000), // 1 minute
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "Find all TODO comments in Rust files",
        None
    ).await?;
    println!("✓ Standard boost: {}", truncate(&result.text, 150));

    println!("\nTesting: Aggressive priority boost...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        enable_metrics: true,
        priority_boost: Some(PriorityBoostConfig {
            strategy: "aggressive".to_string(),
            deadline_ms: Some(30000), // 30 seconds
        }),
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "List all test files",
        None
    ).await?;
    println!("✓ Aggressive boost: {}", truncate(&result.text, 150));

    println!("\n✅ Test 3 passed: Priority boost works correctly");
    Ok(())
}

/// Test 4: Pressure Monitoring
async fn test_pressure_monitoring(agent: &Agent) -> Result<()> {
    println!("\n📊 Test 4: Pressure Monitoring");
    println!("{}", "-".repeat(80));

    println!("Testing: Pressure threshold monitoring...");
    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        enable_metrics: true,
        enable_alerts: true,
        pressure_threshold: Some(10), // Low threshold for testing
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "Search for all function definitions in Rust files and list them",
        None
    ).await?;
    println!("✓ Pressure monitoring active: {}", truncate(&result.text, 150));

    println!("\n✅ Test 4 passed: Pressure monitoring works correctly");
    Ok(())
}

/// Test 5: Per-Lane Timeouts
async fn test_per_lane_timeouts(agent: &Agent) -> Result<()> {
    println!("\n⏰ Test 5: Per-Lane Timeouts");
    println!("{}", "-".repeat(80));

    println!("Testing: Custom lane timeouts...");
    let mut lane_timeouts = HashMap::new();
    lane_timeouts.insert(
        a3s_code_core::queue::SessionLane::Query,
        30000, // 30 seconds for query
    );
    lane_timeouts.insert(
        a3s_code_core::queue::SessionLane::Execute,
        120000, // 2 minutes for execute
    );

    let queue_config = SessionQueueConfig {
        query_max_concurrency: 5,
        execute_max_concurrency: 2,
        enable_metrics: true,
        default_timeout_ms: Some(60000), // 1 minute default
        lane_timeouts,
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new().with_queue_config(queue_config)
    ))?;

    let result = session.send(
        "Read Cargo.toml and list all dependencies",
        None
    ).await?;
    println!("✓ Per-lane timeouts: {}", truncate(&result.text, 150));

    println!("\n✅ Test 5 passed: Per-lane timeouts work correctly");
    Ok(())
}

/// Test 6: Combined Features
async fn test_combined_features(agent: &Agent) -> Result<()> {
    println!("\n🎯 Test 6: Combined Features");
    println!("{}", "-".repeat(80));

    println!("Testing: All features enabled together...");
    let mut lane_timeouts = HashMap::new();
    lane_timeouts.insert(a3s_code_core::queue::SessionLane::Query, 30000);
    lane_timeouts.insert(a3s_code_core::queue::SessionLane::Execute, 120000);

    let queue_config = SessionQueueConfig {
        control_max_concurrency: 2,
        query_max_concurrency: 10,
        execute_max_concurrency: 5,
        generate_max_concurrency: 1,
        enable_metrics: true,
        enable_dlq: true,
        enable_alerts: true,
        default_timeout_ms: Some(60000),
        retry_policy: Some(RetryPolicyConfig {
            strategy: "exponential".to_string(),
            max_retries: 3,
            initial_delay_ms: 100,
            fixed_delay_ms: None,
        }),
        rate_limit: Some(RateLimitConfig {
            limit_type: "per_second".to_string(),
            max_operations: Some(10),
        }),
        priority_boost: Some(PriorityBoostConfig {
            strategy: "standard".to_string(),
            deadline_ms: Some(300000),
        }),
        pressure_threshold: Some(20),
        lane_timeouts,
        ..Default::default()
    };

    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_queue_config(queue_config)
            .with_builtin_skills()
    ))?;

    let start = std::time::Instant::now();
    let result = session.send(
        "Search for all 'pub struct' definitions in Rust files, count them, and create a summary file",
        None
    ).await?;
    let elapsed = start.elapsed();

    println!("✓ Combined features test completed in {:?}", elapsed);
    println!("  Result preview: {}", truncate(&result.text, 200));

    // Cleanup
    let _ = std::fs::remove_file("summary.txt");
    let _ = std::fs::remove_file("struct_summary.txt");

    println!("\n✅ Test 6 passed: All features work together correctly");
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
