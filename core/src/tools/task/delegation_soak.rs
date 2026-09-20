//! S-PD-01: repeated parent fan-out stays inside the parallel cap, and
//! cancelling halfway leaves no child and no source mutation.

use super::super::{ParallelTaskTool, TaskExecutor, MAX_PARALLEL_TASKS_PER_CALL};
use crate::llm::{LlmClient, LlmResponse, Message, ToolDefinition};
use crate::subagent::AgentRegistry;
use crate::subagent_task_tracker::InMemorySubagentTaskTracker;
use crate::tools::types::{Tool, ToolContext};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const PARENT_RUNS: usize = 20;
const RUNTIME_CAP: usize = 8;

struct HoldingClient {
    started: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for HoldingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.started.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming is not used by the delegation soak")
    }
}

fn worker_registry() -> Arc<AgentRegistry> {
    let registry = AgentRegistry::new();
    registry.register(
        crate::subagent::WorkerAgentSpec::custom("worker", "Hold until cancel")
            .with_prompt("Do not write files.")
            .with_max_steps(1)
            .into_agent_definition(),
    );
    Arc::new(registry)
}

fn source_digest(root: &std::path::Path) -> String {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                files.push((path, bytes));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&bytes);
    }
    format!("{:x}", hasher.finalize())
}

#[tokio::test]
#[ignore = "S-PD-01 repeated parent fan-out; run with --ignored"]
async fn soak_parent_fanout_cancel_leaves_no_child_or_source_change() {
    let workspace = tempfile::tempdir().unwrap();
    let before = source_digest(workspace.path());
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let client = Arc::new(HoldingClient {
        started: AtomicUsize::new(0),
    });
    let executor = Arc::new(
        TaskExecutor::new(
            worker_registry(),
            Arc::clone(&client) as Arc<dyn LlmClient>,
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(RUNTIME_CAP)
        .with_subagent_tracker(Arc::clone(&tracker)),
    );
    let tool = Arc::new(ParallelTaskTool::new(executor));
    let tasks: Vec<serde_json::Value> = (0..MAX_PARALLEL_TASKS_PER_CALL)
        .map(|index| {
            serde_json::json!({
                "agent": "worker",
                "description": format!("branch {index}"),
                "prompt": "wait"
            })
        })
        .collect();
    assert!(
        tasks.len() > RUNTIME_CAP,
        "the requested fan-out must exceed the runtime cap"
    );

    for cycle in 0..PARENT_RUNS {
        let started_before = client.started.load(Ordering::SeqCst);
        let cancellation = CancellationToken::new();
        let context = ToolContext::new(workspace.path().to_path_buf())
            .with_session_id(format!("parent-{cycle}"))
            .with_cancellation(cancellation.clone());
        let arguments = serde_json::json!({ "tasks": tasks });
        let run = tokio::spawn({
            let tool = tool.clone();
            async move { tool.execute(&arguments, &context).await }
        });

        let admitted = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let admitted = client.started.load(Ordering::SeqCst) - started_before;
                if admitted >= RUNTIME_CAP {
                    return admitted;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the parent should admit children up to the cap");
        assert!(
            admitted <= RUNTIME_CAP,
            "cycle {cycle} admitted {admitted} children above cap {RUNTIME_CAP}"
        );

        cancellation.cancel();
        let output = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("cancelling halfway must settle the parent")
            .expect("task join")
            .expect("parallel_task returns a result");
        assert!(
            !output.success,
            "cycle {cycle} should not succeed after cancel"
        );
        assert!(tracker.list_pending().await.is_empty());
        let admitted_total = client.started.load(Ordering::SeqCst) - started_before;
        assert!(
            admitted_total <= RUNTIME_CAP,
            "cycle {cycle} left {admitted_total} admissions above the cap"
        );
    }

    assert!(tracker.list_pending().await.is_empty());
    assert_eq!(source_digest(workspace.path()), before);
}
