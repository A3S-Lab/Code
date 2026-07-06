use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_code_core::llm::{StreamEvent, ToolDefinition, ToolResultContent, ToolResultContentField};
use a3s_code_core::{
    Agent, CodeConfig, ContentBlock, LlmClient, LlmResponse, LocalWorkspaceManifest,
    ManifestWorkspaceBackend, Message, PlanningMode, SessionOptions, TokenUsage, WorkspaceServices,
};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ScriptedLlmClient {
    responses: Arc<Mutex<Vec<LlmResponse>>>,
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl ScriptedLlmClient {
    fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(Mutex::new(responses)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<Vec<Message>> {
        self.calls.lock().unwrap().clone()
    }

    fn next_response(&self) -> Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted LLM client exhausted"))
    }
}

#[async_trait]
impl LlmClient for ScriptedLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.calls.lock().unwrap().push(messages.to_vec());
        self.next_response()
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.calls.lock().unwrap().push(messages.to_vec());
        let response = self.next_response()?;
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

fn test_config() -> CodeConfig {
    CodeConfig::from_acl(
        r#"
            default_model = "openai/gpt-4o"

            providers "openai" {
              api_key = "sk-test"

              models "gpt-4o" {
                name = "GPT-4o"
              }
            }
        "#,
    )
    .expect("valid test config")
}

fn text_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn tool_response(tool_id: &str, tool_name: &str, input: serde_json::Value) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input,
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("tool_use".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, content).expect("write fixture file");
}

async fn wait_for_manifest_ready(manifest: &LocalWorkspaceManifest) {
    if manifest.snapshot().version > 0 {
        return;
    }

    let mut rx = manifest.subscribe();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = rx.recv().await.expect("manifest update");
            if snapshot.version > 0 {
                break;
            }
        }
    })
    .await
    .expect("manifest initial scan");
}

fn tool_result_text(messages: &[Vec<Message>]) -> String {
    let mut out = String::new();
    for message in messages.iter().flatten() {
        for block in &message.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                match content {
                    ToolResultContentField::Text(text) => out.push_str(text),
                    ToolResultContentField::Blocks(blocks) => {
                        for block in blocks {
                            if let ToolResultContent::Text { text } = block {
                                out.push_str(text);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

async fn run_glob_agent_task(
    agent: &Agent,
    workspace: &Path,
    services: Arc<WorkspaceServices>,
    pattern: &str,
) -> Result<(Duration, String)> {
    let client = Arc::new(ScriptedLlmClient::new(vec![
        tool_response(
            "call-glob",
            "glob",
            serde_json::json!({
                "pattern": pattern
            }),
        ),
        text_response("Workspace manifest agent task completed."),
    ]));
    let session = agent.session(
        workspace.display().to_string(),
        Some(
            SessionOptions::new()
                .with_llm_client(client.clone())
                .with_workspace_backend(services)
                .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
                .with_planning_mode(PlanningMode::Disabled)
                .with_continuation(false)
                .with_max_tool_rounds(4),
        ),
    )?;

    let start = Instant::now();
    let result = session
        .send(
            "Find the target workspace file and report completion.",
            None,
        )
        .await?;
    let elapsed = start.elapsed();

    assert_eq!(result.tool_calls_count, 1);
    assert_eq!(result.text, "Workspace manifest agent task completed.");
    Ok((elapsed, tool_result_text(&client.calls())))
}

fn median_ms(samples: &[Duration]) -> u128 {
    let mut values = samples
        .iter()
        .map(|duration| duration.as_millis())
        .collect::<Vec<_>>();
    values.sort_unstable();
    values[values.len() / 2]
}

fn sample_ms(samples: &[Duration]) -> Vec<u128> {
    samples
        .iter()
        .map(|duration| duration.as_millis())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_completes_glob_task_through_manifest_workspace_backend() {
    let workspace = tempfile::tempdir().expect("workspace");
    write_file(&workspace.path().join("src/main.rs"), "fn main() {}\n");
    write_file(
        &workspace.path().join("crates/demo/src/lib.rs"),
        "pub fn demo() {}\n",
    );

    let manifest_backend = ManifestWorkspaceBackend::new(workspace.path());
    wait_for_manifest_ready(&manifest_backend.manifest()).await;
    let services = WorkspaceServices::local_with_manifest_backend(manifest_backend);

    let agent = Agent::from_config(test_config()).await.expect("agent");
    let (_elapsed, tool_result) =
        run_glob_agent_task(&agent, workspace.path(), services, "**/*.rs")
            .await
            .expect("agent task");

    assert!(tool_result.contains("src/main.rs"), "{tool_result}");
    assert!(
        tool_result.contains("crates/demo/src/lib.rs"),
        "{tool_result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_glob_task_sees_recent_file_first() {
    let workspace = tempfile::tempdir().expect("workspace");
    write_file(&workspace.path().join("src/a.rs"), "pub fn a() {}\n");
    write_file(&workspace.path().join("src/z.rs"), "pub fn z() {}\n");

    let manifest_backend = ManifestWorkspaceBackend::new(workspace.path());
    let manifest = manifest_backend.manifest();
    wait_for_manifest_ready(&manifest).await;
    manifest.touch_file("src/z.rs");
    let services = WorkspaceServices::local_with_manifest_backend(manifest_backend);

    let agent = Agent::from_config(test_config()).await.expect("agent");
    let (_elapsed, tool_result) =
        run_glob_agent_task(&agent, workspace.path(), services, "**/*.rs")
            .await
            .expect("agent task");

    let hot = tool_result
        .find("src/z.rs")
        .expect("hot file in tool result");
    let cold = tool_result
        .find("src/a.rs")
        .expect("cold file in tool result");
    assert!(
        hot < cold,
        "recent file should be surfaced first in tool result: {tool_result}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "prints filesystem-sensitive agent task latency metrics"]
async fn agent_glob_task_perf_manifest_vs_local_workspace() {
    let file_count = std::env::var("A3S_WORKSPACE_MANIFEST_PERF_FILES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8_000);
    let runs = std::env::var("A3S_WORKSPACE_MANIFEST_PERF_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(7)
        .max(1);

    let workspace = tempfile::tempdir().expect("workspace");
    for shard in 0..64 {
        std::fs::create_dir_all(
            workspace
                .path()
                .join(format!("packages/pkg_{shard:02}/src")),
        )
        .expect("create shard");
    }
    for index in 0..file_count {
        let shard = index % 64;
        let name = if index + 1 == file_count {
            "needle_target.rs".to_string()
        } else {
            format!("file_{index:05}.rs")
        };
        write_file(
            &workspace
                .path()
                .join(format!("packages/pkg_{shard:02}/src/{name}")),
            &format!("pub const VALUE_{index}: usize = {index};\n"),
        );
    }

    let agent = Agent::from_config(test_config()).await.expect("agent");
    let local_services = WorkspaceServices::local(workspace.path());
    let manifest_backend = ManifestWorkspaceBackend::new(workspace.path());
    let scan_start = Instant::now();
    wait_for_manifest_ready(&manifest_backend.manifest()).await;
    let manifest_scan_ms = scan_start.elapsed().as_millis();
    let manifest_services = WorkspaceServices::local_with_manifest_backend(manifest_backend);
    let pattern = "**/needle_target.rs";

    let (_, local_tool_result) =
        run_glob_agent_task(&agent, workspace.path(), local_services.clone(), pattern)
            .await
            .expect("local warmup");
    assert!(local_tool_result.contains("needle_target.rs"));
    let (_, manifest_tool_result) =
        run_glob_agent_task(&agent, workspace.path(), manifest_services.clone(), pattern)
            .await
            .expect("manifest warmup");
    assert!(manifest_tool_result.contains("needle_target.rs"));

    let mut local_samples = Vec::with_capacity(runs);
    let mut manifest_samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        local_samples.push(
            run_glob_agent_task(&agent, workspace.path(), local_services.clone(), pattern)
                .await
                .expect("local run")
                .0,
        );
        manifest_samples.push(
            run_glob_agent_task(&agent, workspace.path(), manifest_services.clone(), pattern)
                .await
                .expect("manifest run")
                .0,
        );
    }

    let local_median = median_ms(&local_samples);
    let manifest_median = median_ms(&manifest_samples);
    let speedup = if manifest_median == 0 {
        f64::INFINITY
    } else {
        local_median as f64 / manifest_median as f64
    };
    println!(
        "workspace_manifest_agent_perf tool=glob files={} runs={} manifest_scan_ms={} \
         no_manifest_median_ms={} manifest_warm_median_ms={} speedup_x={:.2} \
         no_manifest_samples_ms={:?} manifest_samples_ms={:?}",
        file_count,
        runs,
        manifest_scan_ms,
        local_median,
        manifest_median,
        speedup,
        sample_ms(&local_samples),
        sample_ms(&manifest_samples)
    );
}
