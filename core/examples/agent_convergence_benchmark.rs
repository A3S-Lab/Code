//! Deterministic end-to-end benchmark for agent convergence and crash recovery.
//!
//! Run with:
//! `cargo run -p a3s-code-core --example agent_convergence_benchmark --release`

use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::llm::{StreamEvent, ToolDefinition};
use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{
    Agent, CodeConfig, ContentBlock, LlmClient, LlmResponse, Message, PlanningMode, SessionOptions,
    TokenUsage,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const INPUT_USD_PER_MILLION: f64 = 2.50;
const OUTPUT_USD_PER_MILLION: f64 = 10.00;

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<VecDeque<LlmResponse>>>,
    calls: Arc<AtomicUsize>,
    tool_counts: Arc<Mutex<Vec<usize>>>,
}

impl ScriptedClient {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            calls: Arc::new(AtomicUsize::new(0)),
            tool_counts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn tool_counts(&self) -> Vec<usize> {
        self.tool_counts
            .lock()
            .expect("scripted tool-count lock poisoned")
            .clone()
    }

    fn record_tools(&self, tools: &[ToolDefinition]) {
        self.tool_counts
            .lock()
            .expect("scripted tool-count lock poisoned")
            .push(tools.len());
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn next(&self) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses
            .lock()
            .expect("scripted response lock poisoned")
            .pop_front()
            .context("scripted LLM response exhausted")
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.record_tools(tools);
        self.next()
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.record_tools(tools);
        let response = self.next()?;
        let (tx, rx) = mpsc::channel(4);
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

#[derive(Debug, Serialize)]
struct CaseResult {
    name: &'static str,
    passed: bool,
    expected_outcome: &'static str,
    latency_ms: f64,
    llm_calls: usize,
    tool_attempts: usize,
    executed_tool_calls: usize,
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    estimated_cost_usd: f64,
    detail: String,
}

struct Measurements<'a> {
    started: Instant,
    llm_calls: usize,
    tool_attempts: usize,
    executed_tool_calls: usize,
    usage: &'a TokenUsage,
}

impl CaseResult {
    fn measured(
        name: &'static str,
        expected_outcome: &'static str,
        measurements: Measurements<'_>,
        passed: bool,
        detail: impl Into<String>,
    ) -> Self {
        let Measurements {
            started,
            llm_calls,
            tool_attempts,
            executed_tool_calls,
            usage,
        } = measurements;
        let estimated_cost_usd = usage.prompt_tokens as f64 * INPUT_USD_PER_MILLION / 1_000_000.0
            + usage.completion_tokens as f64 * OUTPUT_USD_PER_MILLION / 1_000_000.0;
        Self {
            name,
            passed,
            expected_outcome,
            latency_ms: started.elapsed().as_secs_f64() * 1_000.0,
            llm_calls,
            tool_attempts,
            executed_tool_calls,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            estimated_cost_usd,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    deterministic: bool,
    passed: bool,
    cases: Vec<CaseResult>,
    passed_cases: usize,
    total_cases: usize,
    completion_rate: f64,
    successful_task_rate: f64,
    total_llm_calls: usize,
    total_tool_attempts: usize,
    total_executed_tool_calls: usize,
    total_tokens: usize,
    estimated_cost_usd: f64,
    total_latency_ms: f64,
}

fn config() -> CodeConfig {
    let mut config = CodeConfig::from_acl(
        r#"
        default_model = "openai/benchmark"
        providers "openai" {
          api_key = "deterministic-benchmark"
          models "benchmark" { name = "Deterministic Benchmark" }
        }
        "#,
    )
    .expect("benchmark config must be valid");
    // This benchmark measures foreground convergence and crash recovery. LLM
    // memory extraction is an intentionally detached background workload and
    // would make provider-call counts depend on task scheduling.
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    config
}

fn response(content: Vec<ContentBlock>, stop_reason: &str) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content,
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
        },
        usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some(stop_reason.to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn text_response(text: &str) -> LlmResponse {
    response(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        "end_turn",
    )
}

fn tool_response(id: &str, name: &str, input: Value) -> LlmResponse {
    response(
        vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }],
        "tool_use",
    )
}

fn base_options(client: Arc<dyn LlmClient>) -> SessionOptions {
    SessionOptions::new()
        .with_llm_client(client)
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_planning_mode(PlanningMode::Disabled)
        .with_max_tool_rounds(16)
}

async fn successful_tool_task(agent: &Agent) -> Result<CaseResult> {
    let workspace = tempfile::tempdir()?;
    std::fs::write(workspace.path().join("target.txt"), "benchmark evidence\n")?;
    let client = Arc::new(ScriptedClient::new(vec![
        tool_response(
            "glob-1",
            "search",
            json!({"mode": "glob", "query": "**/*.txt"}),
        ),
        text_response("Task completed with workspace evidence."),
    ]));
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(base_options(client.clone()).with_continuation(false)),
        )
        .await?;
    let started = Instant::now();
    let result = session
        .send("Find the text fixture and finish.", None)
        .await?;
    let passed = result.text == "Task completed with workspace evidence."
        && result.tool_calls_count == 1
        && client.call_count() == 2;
    Ok(CaseResult::measured(
        "successful_tool_task",
        "complete_after_one_tool",
        Measurements {
            started,
            llm_calls: client.call_count(),
            tool_attempts: 1,
            executed_tool_calls: result.tool_calls_count,
            usage: &result.usage,
        },
        passed,
        result.text,
    ))
}

async fn incomplete_response_converges(agent: &Agent) -> Result<CaseResult> {
    let client = Arc::new(ScriptedClient::new(vec![
        text_response("Let me inspect the code..."),
        text_response("must not be consumed"),
    ]));
    let session = agent
        .session_async(
            "/tmp/a3s-convergence-benchmark".to_string(),
            Some(
                base_options(client.clone())
                    .with_continuation(true)
                    .with_max_continuation_turns(20),
            ),
        )
        .await?;
    let started = Instant::now();
    let result = session.send("Inspect the workspace.", None).await?;
    let passed = result.text == "Let me inspect the code..."
        && result.tool_calls_count == 0
        && client.call_count() == 1;
    Ok(CaseResult::measured(
        "incomplete_response_convergence",
        "return_the_model_turn_without_a_continuation",
        Measurements {
            started,
            llm_calls: client.call_count(),
            tool_attempts: 0,
            executed_tool_calls: result.tool_calls_count,
            usage: &result.usage,
        },
        passed,
        result.text,
    ))
}

async fn tool_round_cap_stops_repeated_tools(agent: &Agent) -> Result<CaseResult> {
    let workspace = tempfile::tempdir()?;
    let input = json!({"mode": "grep", "query": "never-matches"});
    let client = Arc::new(ScriptedClient::new(vec![
        tool_response("grep-1", "search", input.clone()),
        tool_response("grep-2", "search", input.clone()),
        text_response("Stopped after the tool-round cap."),
        text_response("must not be consumed"),
    ]));
    let options = base_options(client.clone())
        .with_permission_policy(PermissionPolicy::new().allow("search(*)"))
        .with_max_tool_rounds(2);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await?;
    let started = Instant::now();
    let result = session
        .send("Repeat the same search forever.", None)
        .await?;
    let tool_counts = client.tool_counts();
    let passed = result.text == "Stopped after the tool-round cap."
        && result.tool_calls_count == 2
        && client.call_count() == 3
        && tool_counts.len() == 3
        && tool_counts[0] > 0
        && tool_counts[1] > 0
        && tool_counts[2] == 0;
    Ok(CaseResult::measured(
        "tool_round_cap",
        "one_final_completion_with_an_empty_tool_list",
        Measurements {
            started,
            llm_calls: client.call_count(),
            tool_attempts: 2,
            executed_tool_calls: result.tool_calls_count,
            usage: &result.usage,
        },
        passed,
        result.text,
    ))
}

async fn checkpoint_resume_preserves_accounting(agent: &Agent) -> Result<CaseResult> {
    let store = Arc::new(MemorySessionStore::new());
    let store_api: Arc<dyn SessionStore> = store.clone();
    let run_id = "deterministic-benchmark-checkpoint";
    store_api
        .save_loop_checkpoint(
            run_id,
            &LoopCheckpoint {
                schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: run_id.to_string(),
                session_id: "deterministic-benchmark-resume".to_string(),
                capability_binding: None,
                turn: 3,
                messages: vec![Message::user("Continue the interrupted task.")],
                total_usage: TokenUsage {
                    prompt_tokens: 80,
                    completion_tokens: 20,
                    total_tokens: 100,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                tool_calls_count: 3,
                verification_reports: Vec::new(),
                convergence: Default::default(),
                checkpoint_ms: 1_700_000_000_000,
            },
        )
        .await?;
    let client = Arc::new(ScriptedClient::new(vec![text_response(
        "Resumed task completed.",
    )]));
    let options = base_options(client.clone())
        .with_session_id("deterministic-benchmark-resume")
        .with_session_store(store_api);
    let session = agent
        .session_async("/tmp/a3s-resume-benchmark".to_string(), Some(options))
        .await?;
    let started = Instant::now();
    let result = session.resume_run(run_id).await?;
    let passed = result.text == "Resumed task completed."
        && result.usage.prompt_tokens == 90
        && result.usage.completion_tokens == 25
        && result.usage.total_tokens == 115
        && result.tool_calls_count == 3;
    Ok(CaseResult::measured(
        "checkpoint_resume_accounting",
        "resume_without_resetting_budgets_or_metrics",
        Measurements {
            started,
            llm_calls: client.call_count(),
            tool_attempts: 3,
            executed_tool_calls: result.tool_calls_count,
            usage: &result.usage,
        },
        passed,
        result.text,
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let agent = Agent::from_config(config()).await?;
    let cases = vec![
        successful_tool_task(&agent).await?,
        incomplete_response_converges(&agent).await?,
        tool_round_cap_stops_repeated_tools(&agent).await?,
        checkpoint_resume_preserves_accounting(&agent).await?,
    ];
    let passed_cases = cases.iter().filter(|case| case.passed).count();
    let successful_tasks = cases
        .iter()
        .filter(|case| {
            case.name == "successful_tool_task" || case.name == "checkpoint_resume_accounting"
        })
        .filter(|case| case.passed)
        .count();
    let passed = passed_cases == cases.len();
    let report = BenchmarkReport {
        schema_version: 1,
        deterministic: true,
        passed,
        passed_cases,
        total_cases: cases.len(),
        completion_rate: passed_cases as f64 / cases.len() as f64,
        successful_task_rate: successful_tasks as f64 / 2.0,
        total_llm_calls: cases.iter().map(|case| case.llm_calls).sum(),
        total_tool_attempts: cases.iter().map(|case| case.tool_attempts).sum(),
        total_executed_tool_calls: cases.iter().map(|case| case.executed_tool_calls).sum(),
        total_tokens: cases.iter().map(|case| case.total_tokens).sum(),
        estimated_cost_usd: cases.iter().map(|case| case.estimated_cost_usd).sum(),
        total_latency_ms: cases.iter().map(|case| case.latency_ms).sum(),
        cases,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.passed {
        anyhow::bail!(
            "agent convergence benchmark failed: {passed_cases}/{} cases passed",
            report.total_cases
        );
    }
    Ok(())
}
