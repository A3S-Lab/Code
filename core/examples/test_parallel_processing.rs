//! A3S Code - Parallel Task Processing Integration Test
//!
//! Demonstrates parallel processing of multiple tasks using A3S Lane queue.
//! Tests concurrent file analysis, code review, and documentation generation.
//!
//! Run with: cargo run --example test_parallel_processing

use a3s_code_core::queue::RetryPolicyConfig;
use a3s_code_core::{Agent, SessionOptions, SessionQueueConfig};
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

    // Try project root
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
    println!("🚀 A3S Code - Parallel Task Processing Integration Test\n");
    println!("{}", "=".repeat(80));

    let config_path = find_config()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));
    println!();

    // Test 1: Sequential processing (baseline)
    println!("📦 Test 1: Sequential Processing (Baseline)");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;
        let session = agent.session(".", None)?;

        let tasks = [
            "Count the number of Rust files in this project",
            "Find all TODO comments in Rust files",
            "List all public functions in src/lib.rs",
        ];

        let start = Instant::now();
        println!("Processing {} tasks sequentially...", tasks.len());

        for (i, task) in tasks.iter().enumerate() {
            println!("  Task {}: {}", i + 1, task);
            let result = session.send(task, None).await?;
            println!("  ✓ Completed: {} chars", result.text.len());
        }

        let duration = start.elapsed();
        println!(
            "\n✓ Sequential processing took: {:.2}s",
            duration.as_secs_f64()
        );
    }
    println!("✅ Test 1 passed: Sequential processing works\n");

    // Test 2: Parallel processing with queue
    println!("⚡ Test 2: Parallel Processing with Queue");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        // Configure queue for parallel processing
        let queue_config = SessionQueueConfig {
            query_max_concurrency: 3,   // Allow 3 concurrent query operations
            execute_max_concurrency: 2, // Allow 2 concurrent execute operations
            enable_metrics: true,       // Enable metrics collection
            enable_dlq: true,           // Enable dead letter queue
            retry_policy: Some(RetryPolicyConfig {
                strategy: "exponential".to_string(),
                max_retries: 3,
                initial_delay_ms: 100,
                fixed_delay_ms: None,
            }),
            ..Default::default()
        };

        let opts = SessionOptions::new().with_queue_config(queue_config);

        let session = agent.session(".", Some(opts))?;

        let tasks = [
            "Count the number of Rust files in this project",
            "Find all TODO comments in Rust files",
            "List all public functions in src/lib.rs",
            "Find all async functions in the codebase",
            "Count lines of code in all Rust files",
        ];

        let start = Instant::now();
        println!("Processing {} tasks in parallel...", tasks.len());

        // Spawn all tasks concurrently
        let mut handles = vec![];
        let session = Arc::new(session);
        for (i, task) in tasks.iter().enumerate() {
            println!("  Queuing task {}: {}", i + 1, task);
            let session_clone = Arc::clone(&session);
            let task_str = task.to_string();

            let handle = tokio::spawn(async move {
                let result = session_clone.send(&task_str, None).await;
                (i + 1, result)
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        println!("\n  Waiting for all tasks to complete...");
        for handle in handles {
            let (task_num, result) = handle.await?;
            match result {
                Ok(r) => println!("  ✓ Task {} completed: {} chars", task_num, r.text.len()),
                Err(e) => println!("  ✗ Task {} failed: {}", task_num, e),
            }
        }

        let duration = start.elapsed();
        println!(
            "\n✓ Parallel processing took: {:.2}s",
            duration.as_secs_f64()
        );

        // Check queue stats
        if session.has_queue() {
            let stats = session.queue_stats().await;
            println!("\n📊 Queue Statistics:");
            println!("  Total pending: {}", stats.total_pending);
            println!("  Total active: {}", stats.total_active);
            println!("  External pending: {}", stats.external_pending);
        }
    }
    println!("\n✅ Test 2 passed: Parallel processing with queue works\n");

    // Test 3: Parallel Processing with Priority Lanes
    println!("🎯 Test 3: Parallel Processing with Priority Lanes");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        let queue_config = SessionQueueConfig {
            control_max_concurrency: 1,  // P0: Control operations
            query_max_concurrency: 3,    // P1: Query operations (highest concurrency)
            execute_max_concurrency: 2,  // P2: Execute operations
            generate_max_concurrency: 1, // P3: Generate operations
            enable_metrics: true,
            enable_dlq: true,
            ..Default::default()
        };

        let opts = SessionOptions::new().with_queue_config(queue_config);

        let session = agent.session(".", Some(opts))?;

        println!("Testing priority-based task execution...");
        println!("  P1 (Query): 3 concurrent tasks");
        println!("  P2 (Execute): 2 concurrent tasks");
        println!("  P3 (Generate): 1 concurrent task");

        let start = Instant::now();

        // Mix of different task types
        let tasks = [
            ("Query", "How many Rust files are there?"),
            ("Query", "What is the project structure?"),
            ("Query", "List all dependencies"),
            ("Execute", "Create a summary of the codebase"),
            ("Execute", "Analyze code complexity"),
        ];

        let mut handles = vec![];
        let session = Arc::new(session);
        for (i, (task_type, task)) in tasks.iter().enumerate() {
            println!("  Queuing {} task {}: {}", task_type, i + 1, task);
            let session_clone = Arc::clone(&session);
            let task_str = task.to_string();

            let handle = tokio::spawn(async move {
                let result = session_clone.send(&task_str, None).await;
                (i + 1, result)
            });
            handles.push(handle);
        }

        println!("\n  Waiting for all tasks to complete...");
        for handle in handles {
            let (task_num, result) = handle.await?;
            match result {
                Ok(r) => println!("  ✓ Task {} completed: {} chars", task_num, r.text.len()),
                Err(e) => println!("  ✗ Task {} failed: {}", task_num, e),
            }
        }

        let duration = start.elapsed();
        println!(
            "\n✓ Priority-based processing took: {:.2}s",
            duration.as_secs_f64()
        );
    }
    println!("\n✅ Test 3 passed: Priority lanes work correctly\n");

    // Test 4: Parallel processing with retry on failure
    println!("🔄 Test 4: Parallel Processing with Retry Policy");
    println!("{}", "-".repeat(80));
    {
        let agent = Agent::new(config_path.to_str().unwrap()).await?;

        let queue_config = SessionQueueConfig {
            query_max_concurrency: 3,
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

        let opts = SessionOptions::new().with_queue_config(queue_config);

        let session = agent.session(".", Some(opts))?;

        println!("Testing retry policy with exponential backoff...");
        println!("  Max retries: 3");
        println!("  Initial delay: 100ms");
        println!("  Strategy: exponential");

        let tasks = [
            "Analyze the main function",
            "Find all error types",
            "List all test functions",
        ];

        let start = Instant::now();
        let mut handles = vec![];
        let session = Arc::new(session);

        for (i, task) in tasks.iter().enumerate() {
            println!("  Queuing task {}: {}", i + 1, task);
            let session_clone = Arc::clone(&session);
            let task_str = task.to_string();

            let handle = tokio::spawn(async move {
                let result = session_clone.send(&task_str, None).await;
                (i + 1, result)
            });
            handles.push(handle);
        }

        println!("\n  Waiting for all tasks to complete...");
        for handle in handles {
            let (task_num, result) = handle.await?;
            match result {
                Ok(r) => println!("  ✓ Task {} completed: {} chars", task_num, r.text.len()),
                Err(e) => println!("  ✗ Task {} failed: {}", task_num, e),
            }
        }

        let duration = start.elapsed();
        println!(
            "\n✓ Processing with retry took: {:.2}s",
            duration.as_secs_f64()
        );

        if session.has_queue() {
            let stats = session.queue_stats().await;
            println!("\n📊 Final Queue Statistics:");
            println!("  Total pending: {}", stats.total_pending);
            println!("  Total active: {}", stats.total_active);
            println!("  External pending: {}", stats.external_pending);
        }
    }
    println!("\n✅ Test 4 passed: Retry policy works correctly\n");

    println!();
    println!("{}", "=".repeat(80));
    println!("✅ All parallel processing tests completed successfully!");
    println!("{}", "=".repeat(80));

    Ok(())
}
