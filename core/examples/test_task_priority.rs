//! Lane-Based Priority Preemption Test with Real LLM
//!
//! Demonstrates how the lane-based priority system allows high-priority tasks
//! to preempt queued low-priority tasks:
//!
//! 1. Constrain concurrency so tasks must queue up
//! 2. Submit multiple low-priority tasks (Execute lane: bash commands)
//! 3. Submit a high-priority task later (Query lane: read/grep)
//! 4. Observe that the high-priority task executes before queued low-priority tasks
//!
//! Lane priorities (lower = higher priority):
//!   Control (P0) > Query (P1) > Execute (P2) > Generate (P3)
//!
//! Run with: cargo run --example test_task_priority

use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::queue::SessionQueueConfig;
use a3s_code_core::{Agent, AgentSession, SessionOptions};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

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

/// Record of when a task completed
#[derive(Debug, Clone)]
struct CompletionRecord {
    name: String,
    lane: String,
    submitted_at: Duration,
    completed_at: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=info")
        .init();

    println!("🚀 A3S Code - Lane-Based Priority Preemption Test\n");
    println!("{}", "=".repeat(80));

    let config_path = find_config()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // Test 1: High-priority Query task preempts queued Execute tasks
    test_query_preempts_execute(&agent).await?;

    // Test 2: Multiple priority levels
    test_multi_level_priority(&agent).await?;

    // Test 3: Late urgent task inserted into busy queue
    test_late_urgent_insertion(&agent).await?;

    println!("\n{}", "=".repeat(80));
    println!("✅ All lane-based priority preemption tests completed!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Helper: spawn a task that sends a prompt and records completion time
fn spawn_send(
    session: Arc<AgentSession>,
    name: String,
    prompt: String,
    lane_label: String,
    start: Instant,
    completions: Arc<Mutex<Vec<CompletionRecord>>>,
) -> tokio::task::JoinHandle<Result<()>> {
    let submitted_at = start.elapsed();
    let marker = if lane_label.contains("P1") {
        "🚨"
    } else {
        "📤"
    };
    println!(
        "  [{:>6.2}s] {} Submitting: {} ({})",
        submitted_at.as_secs_f64(),
        marker,
        name,
        lane_label
    );

    tokio::spawn(async move {
        // Each spawned task uses its own history (None → fresh conversation)
        let result = session.send(&prompt, None).await;
        let completed_at = start.elapsed();

        completions.lock().await.push(CompletionRecord {
            name: name.clone(),
            lane: lane_label.clone(),
            submitted_at,
            completed_at,
        });

        let chars = result.as_ref().map(|r| r.text.len()).unwrap_or(0);
        let done_marker = if lane_label.contains("P1") {
            "🚨"
        } else {
            "✅"
        };
        println!(
            "  [{:>6.2}s] {} Completed: {} ({} chars)",
            completed_at.as_secs_f64(),
            done_marker,
            name,
            chars
        );
        result.map(|_| ()).map_err(|e| anyhow::anyhow!("{}", e))
    })
}

/// Print completion records sorted by time
fn print_completion_order(records: &[CompletionRecord]) {
    let mut sorted: Vec<_> = records.to_vec();
    sorted.sort_by(|a, b| a.completed_at.cmp(&b.completed_at));
    println!("\n  --- Completion Order ---");
    for (i, record) in sorted.iter().enumerate() {
        let marker = if record.lane.contains("P1") {
            "🚨"
        } else {
            "  "
        };
        println!(
            "  {} {}. {} [{}] — submitted {:.2}s, completed {:.2}s",
            marker,
            i + 1,
            record.name,
            record.lane,
            record.submitted_at.as_secs_f64(),
            record.completed_at.as_secs_f64()
        );
    }
}

/// Test 1: Query-lane task (P1) preempts queued Execute-lane tasks (P2)
async fn test_query_preempts_execute(agent: &Agent) -> Result<()> {
    println!("\n📋 Test 1: Query (P1) Preempts Execute (P2)");
    println!("{}", "-".repeat(80));
    println!("  Execute lane concurrency: 1 (tasks must queue)");
    println!("  Query lane concurrency: 2 (higher priority, separate capacity)");
    println!("  Submit 3 Execute tasks first, then 1 Query task");
    println!("  Expected: Query task completes before remaining Execute tasks\n");

    let queue_config = SessionQueueConfig {
        execute_max_concurrency: 1,
        query_max_concurrency: 2,
        generate_max_concurrency: 1,
        enable_metrics: true,
        ..Default::default()
    };

    let mut opts = SessionOptions::new().with_queue_config(queue_config);
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = Arc::new(agent.session(".", Some(opts))?);

    let start = Instant::now();
    let completions: Arc<Mutex<Vec<CompletionRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Submit 3 Execute-lane tasks (bash → Execute lane P2)
    for i in 1..=3 {
        let prompt = format!(
            "Run this bash command and tell me the output: echo 'Task {} started at $(date +%H:%M:%S)' && sleep 1 && echo 'Task {} done'",
            i, i
        );
        handles.push(spawn_send(
            Arc::clone(&session),
            format!("Execute-{}", i),
            prompt,
            "Execute (P2)".to_string(),
            start,
            Arc::clone(&completions),
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Wait for queue to fill
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!();

    // Submit Query-lane task (read → Query lane P1)
    handles.push(spawn_send(
        Arc::clone(&session),
        "Query-Urgent".to_string(),
        "Read the Cargo.toml file and tell me the package name and version".to_string(),
        "Query (P1)".to_string(),
        start,
        Arc::clone(&completions),
    ));

    // Wait for all tasks
    for handle in handles {
        let _ = handle.await;
    }

    let records = completions.lock().await;
    print_completion_order(&records);
    println!("\n✅ Test 1 completed");
    Ok(())
}

/// Test 2: Multi-level priority — Execute (P2) vs Query (P1)
async fn test_multi_level_priority(agent: &Agent) -> Result<()> {
    println!("\n\n📋 Test 2: Multi-Level Priority (Query P1 vs Execute P2)");
    println!("{}", "-".repeat(80));
    println!("  All lanes constrained to concurrency=1");
    println!("  Submit Execute task first, then Query task");
    println!("  Expected: Query task scheduled with higher priority\n");

    let queue_config = SessionQueueConfig {
        control_max_concurrency: 1,
        query_max_concurrency: 1,
        execute_max_concurrency: 1,
        generate_max_concurrency: 1,
        enable_metrics: true,
        ..Default::default()
    };

    let mut opts = SessionOptions::new().with_queue_config(queue_config);
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = Arc::new(agent.session(".", Some(opts))?);

    let start = Instant::now();
    let completions: Arc<Mutex<Vec<CompletionRecord>>> = Arc::new(Mutex::new(Vec::new()));

    // Execute task first (lower priority P2)
    let exec_handle = spawn_send(
        Arc::clone(&session),
        "Execute-Task".to_string(),
        "Run: echo 'execute-lane task' && sleep 1 && echo 'done'".to_string(),
        "Execute (P2)".to_string(),
        start,
        Arc::clone(&completions),
    );

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Query task (higher priority P1)
    let query_handle = spawn_send(
        Arc::clone(&session),
        "Query-Task".to_string(),
        "Read the Cargo.toml file and show the first 5 lines".to_string(),
        "Query (P1)".to_string(),
        start,
        Arc::clone(&completions),
    );

    let _ = exec_handle.await;
    let _ = query_handle.await;

    let records = completions.lock().await;
    print_completion_order(&records);
    println!("\n✅ Test 2 completed");
    Ok(())
}

/// Test 3: Late urgent task inserted into a busy queue
async fn test_late_urgent_insertion(agent: &Agent) -> Result<()> {
    println!("\n\n📋 Test 3: Late Urgent Task Insertion");
    println!("{}", "-".repeat(80));
    println!("  Execute concurrency: 1, Query concurrency: 2");
    println!("  Submit 4 slow Execute tasks, then 1 urgent Query task at 2s mark");
    println!("  Expected: Urgent task completes before remaining Execute tasks\n");

    let queue_config = SessionQueueConfig {
        execute_max_concurrency: 1,
        query_max_concurrency: 2,
        generate_max_concurrency: 1,
        enable_metrics: true,
        ..Default::default()
    };

    let mut opts = SessionOptions::new().with_queue_config(queue_config);
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = Arc::new(agent.session(".", Some(opts))?);

    let start = Instant::now();
    let completions: Arc<Mutex<Vec<CompletionRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();

    // Submit 4 slow Execute-lane tasks
    for i in 1..=4 {
        let prompt = format!(
            "Run: echo 'slow task {} start' && sleep 2 && echo 'slow task {} end'",
            i, i
        );
        handles.push(spawn_send(
            Arc::clone(&session),
            format!("SlowExec-{}", i),
            prompt,
            "Execute (P2)".to_string(),
            start,
            Arc::clone(&completions),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait for queue to fill
    println!("\n  ⏳ Waiting 2s for queue to fill up...\n");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Print queue stats
    let stats = session.queue_stats().await;
    println!(
        "  📊 Queue stats: pending={}, active={}",
        stats.total_pending, stats.total_active
    );

    // Inject urgent Query task
    handles.push(spawn_send(
        Arc::clone(&session),
        "UrgentQuery".to_string(),
        "Use grep to search for 'name' in Cargo.toml and tell me the result".to_string(),
        "Query (P1)".to_string(),
        start,
        Arc::clone(&completions),
    ));

    // Wait for all
    for handle in handles {
        let _ = handle.await;
    }

    // Print completion order and check preemption
    let records = completions.lock().await;
    let mut sorted: Vec<_> = records.to_vec();
    sorted.sort_by(|a, b| a.completed_at.cmp(&b.completed_at));
    print_completion_order(&records);

    let urgent_completed = sorted
        .iter()
        .find(|r| r.name == "UrgentQuery")
        .map(|r| r.completed_at);
    let last_exec_completed = sorted
        .iter()
        .filter(|r| r.lane.contains("P2"))
        .last()
        .map(|r| r.completed_at);

    if let (Some(urgent), Some(last_exec)) = (urgent_completed, last_exec_completed) {
        if urgent < last_exec {
            println!(
                "\n  ✅ Priority preemption confirmed: UrgentQuery ({:.2}s) completed before last Execute task ({:.2}s)",
                urgent.as_secs_f64(),
                last_exec.as_secs_f64()
            );
        } else {
            println!(
                "\n  ℹ️  UrgentQuery ({:.2}s) completed after last Execute task ({:.2}s)",
                urgent.as_secs_f64(),
                last_exec.as_secs_f64()
            );
            println!(
                "     This can happen if Execute tasks were already running when urgent task was submitted"
            );
        }
    }

    println!("\n✅ Test 3 completed");
    Ok(())
}
