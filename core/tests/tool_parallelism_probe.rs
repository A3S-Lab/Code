//! TOOL-PAR1 measurement: how long one model turn's batch of read-only tool
//! calls takes on the session (FactRun) path.
//!
//! Every call sleeps `LATENCY`. A parallel batch of four takes about one
//! latency; a sequential batch takes about four. The batch time is the gap
//! between the model call that returned the batch and the next model call.
//! The assertions pin the measured behaviour so a change to the scheduler
//! is visible here.
//!
//! Before TOOL-PAR1 only the built-in read allowlist ran in parallel: a host
//! tool declaring `read_only_paginated(4)` took 4.15 latencies for four calls,
//! and a mixed read and host-tool batch took 4.16.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use a3s_code_core::workspace::{WorkspacePath, WorkspaceReadOverlay};
use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const LATENCY: Duration = Duration::from_millis(250);
const BATCH: usize = 4;

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/model".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "model".into(),
                name: "Model".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

/// Each read of a workspace file costs `LATENCY`.
struct SlowReads;

#[async_trait]
impl WorkspaceReadOverlay for SlowReads {
    async fn read_text(&self, path: &WorkspacePath) -> Option<String> {
        tokio::time::sleep(LATENCY).await;
        Some(format!("contents of {}\n", path.as_str()))
    }
}

/// A host tool that declares itself read-only and parallel-safe up to
/// `max_parallelism` concurrent calls.
struct SlowProbe {
    name: &'static str,
    max_parallelism: usize,
}

#[async_trait]
impl Tool for SlowProbe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Look up one key (read-only)."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "key": { "type": "string" } },
            "required": ["key"]
        })
    }
    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities::read_only_paginated(self.max_parallelism)
    }
    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        tokio::time::sleep(LATENCY).await;
        Ok(ToolOutput::success(format!("value of {}", args["key"])))
    }
}

/// First call: the batch. Second call: a final answer.
struct BatchClient {
    batch: Vec<(String, serde_json::Value)>,
    starts: Mutex<Vec<Instant>>,
    returned: Mutex<Vec<Instant>>,
}

impl BatchClient {
    fn response(&self, call: usize) -> LlmResponse {
        let content = if call == 0 {
            self.batch
                .iter()
                .enumerate()
                .map(|(index, (name, input))| ContentBlock::ToolUse {
                    id: format!("call-{index}"),
                    name: name.clone(),
                    input: input.clone(),
                })
                .collect()
        } else {
            vec![ContentBlock::Text {
                text: "done".into(),
            }]
        };
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content,
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some(if call == 0 { "tool_use" } else { "end_turn" }.into()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for BatchClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        Ok(self.response(1))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let call = {
            let mut starts = self.starts.lock().unwrap();
            starts.push(Instant::now());
            starts.len() - 1
        };
        let response = self.response(call);
        self.returned.lock().unwrap().push(Instant::now());
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

/// Runs one batch and returns its wall time.
async fn batch_time(batch: Vec<(String, serde_json::Value)>) -> Duration {
    let workspace = tempfile::tempdir().unwrap();
    for index in 0..BATCH {
        std::fs::write(workspace.path().join(format!("f{index}.txt")), "x\n").unwrap();
    }
    let client = Arc::new(BatchClient {
        batch,
        starts: Mutex::default(),
        returned: Mutex::default(),
    });
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_llm_client(client.clone())
        .with_workspace_read_overlay(Arc::new(SlowReads))
        .with_permission_policy(
            PermissionPolicy::new()
                .allow("read(*)")
                .allow("slow_probe(*)")
                .allow("narrow_probe(*)"),
        )
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();
    for (name, max_parallelism) in [("slow_probe", BATCH), ("narrow_probe", 2)] {
        session
            .register_dynamic_tool(Arc::new(SlowProbe {
                name,
                max_parallelism,
            }))
            .unwrap();
    }

    let (mut events, worker) = session.stream("probe", None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while events.recv().await.is_some() {}
    })
    .await
    .expect("run finished");
    session.close().await;
    worker.abort();

    let starts = client.starts.lock().unwrap().clone();
    let returned = client.returned.lock().unwrap().clone();
    assert!(
        starts.len() >= 2,
        "the batch never reached a second model call"
    );
    starts[1].duration_since(returned[0])
}

fn reads() -> Vec<(String, serde_json::Value)> {
    (0..BATCH)
        .map(|index| {
            (
                "read".to_string(),
                serde_json::json!({ "file_path": format!("f{index}.txt") }),
            )
        })
        .collect()
}

fn probes(name: &str) -> Vec<(String, serde_json::Value)> {
    (0..BATCH)
        .map(|index| {
            (
                name.to_string(),
                serde_json::json!({ "key": format!("k{index}") }),
            )
        })
        .collect()
}

fn report(label: &str, elapsed: Duration) -> f64 {
    let latencies = elapsed.as_secs_f64() / LATENCY.as_secs_f64();
    eprintln!(
        "TOOL-PAR1 {label}: {BATCH} calls x {} ms each took {} ms ({latencies:.2} latencies)",
        LATENCY.as_millis(),
        elapsed.as_millis()
    );
    latencies
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builtin_read_batch_runs_in_parallel() {
    let latencies = report("read x4", batch_time(reads()).await);
    assert!(latencies < 2.0, "the read batch ran sequentially");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_parallel_host_tool_batch_runs_in_parallel() {
    let latencies = report("slow_probe x4", batch_time(probes("slow_probe")).await);
    assert!(
        latencies < 2.0,
        "the declared host-tool batch ran sequentially"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_read_and_host_tool_batch_runs_in_parallel() {
    let mut batch = reads();
    batch.truncate(2);
    batch.extend(probes("slow_probe").into_iter().take(2));
    let latencies = report("read x2 + slow_probe x2", batch_time(batch).await);
    assert!(latencies < 2.0, "the mixed batch ran sequentially");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_above_declared_max_parallelism_runs_sequentially() {
    let latencies = report(
        "narrow_probe x4 (max 2)",
        batch_time(probes("narrow_probe")).await,
    );
    assert!(
        latencies >= 3.5,
        "a batch above max_parallelism ran in parallel"
    );
}
