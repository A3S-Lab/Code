//! Real-LLM coverage for the 9.1.3 context harness: bounded results with a
//! pageable artifact, repeat feedback, long `web_fetch` pages, and the session
//! context view. The scenarios use registered tools rather than `bash`, so they
//! also run on macOS, where native process execution is refused.
//!
//! Each scenario asserts what the model received and did, not only its answer.
//! Run serially with a provider ACL, for example:
//!
//! ```bash
//! A3S_CONFIG_FILE=../../scripts/harbor/native-tb.acl \
//!   cargo test -p a3s-code-core --test test_context_harness_real_llm -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, PlanningMode, SessionOptions, SystemPromptSlots,
};
use serde_json::Value;

const TURN_TIMEOUT: Duration = Duration::from_secs(420);
const GUIDELINES: &str = "This is an integration test. Follow the user's numbered protocol exactly, one tool call at a time, and wait for each result before the next call.";

#[derive(Debug)]
struct Call {
    name: String,
    args: Value,
    output: String,
    exit_code: i32,
}

#[derive(Debug)]
struct Trace {
    calls: Vec<Call>,
    text: String,
    gate: Option<String>,
}

impl Trace {
    fn named(&self, name: &str) -> Vec<&Call> {
        self.calls.iter().filter(|call| call.name == name).collect()
    }

    fn mentions(&self, needle: &str) -> bool {
        self.calls
            .iter()
            .any(|call| call.args.to_string().contains(needle))
    }
}

async fn session(workspace: &std::path::Path, id: &str, rules: &[&str]) -> AgentSession {
    let agent = Agent::from_config(support::layer_c_model::load_pinned_layer_c_config())
        .await
        .expect("agent from provider config");
    let mut policy = PermissionPolicy::new().allow_all(rules);
    policy.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id(id)
        .with_permission_policy(policy)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(12)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(GUIDELINES));
    agent
        .session_async(workspace.display().to_string(), Some(options))
        .await
        .expect("session")
}

async fn run(session: &AgentSession, prompt: &str) -> Trace {
    let (mut rx, handle) = session.stream(prompt, None).await.expect("stream");
    let mut starts = HashMap::<String, Value>::new();
    let mut calls = Vec::new();
    let outcome = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match rx.recv().await.expect("stream closed before End") {
                AgentEvent::ToolExecutionStart { id, args, .. } => {
                    starts.insert(id, args);
                }
                AgentEvent::ToolEnd {
                    id,
                    name,
                    args,
                    output,
                    exit_code,
                    ..
                } => {
                    let args = starts.remove(&id).or(args).unwrap_or(Value::Null);
                    eprintln!(
                        "[harness-real] {name} {args} -> exit {exit_code}, {} bytes",
                        output.len()
                    );
                    calls.push(Call {
                        name,
                        args,
                        output,
                        exit_code,
                    });
                }
                AgentEvent::End { text, .. } => break (text, None),
                AgentEvent::Error { message } if message.starts_with("completion gate:") => {
                    break (String::new(), Some(message));
                }
                AgentEvent::Error { message } => panic!("stream error: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await;
    let Ok((text, gate)) = outcome else {
        handle.abort();
        panic!("turn timed out");
    };
    let _ = tokio::time::timeout(Duration::from_secs(30), handle).await;
    eprintln!("[harness-real] final: {text}\n[harness-real] gate: {gate:?}");
    Trace { calls, text, gate }
}

/// Fixed text, long enough to be bounded, with one line in the omitted middle.
struct DumpRows;

#[async_trait::async_trait]
impl Tool for DumpRows {
    fn name(&self) -> &str {
        "dump_rows"
    }

    fn description(&self) -> &str {
        "Print the diagnostic table"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let mut rows = String::new();
        for row in 1..=4000 {
            rows.push_str(&format!("row-{row}-filler-filler-filler\n"));
            if row == 2000 {
                rows.push_str("NEEDLE_VALUE=K7Q2-PLUM\n");
            }
        }
        Ok(ToolOutput::success(rows))
    }
}

/// A status that never changes, polled by name.
struct PollStatus(AtomicUsize);

#[async_trait::async_trait]
impl Tool for PollStatus {
    fn name(&self) -> &str {
        "poll_status"
    }

    fn description(&self) -> &str {
        "Report the deployment status"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("state=pending"))
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_recovers_an_omitted_middle_from_the_artifact_and_reports_context() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(
        workspace.path(),
        "harness-real-artifact",
        &["dump_rows(*)", "read(*)", "search(*)"],
    )
    .await;
    session.register_dynamic_tool(Arc::new(DumpRows)).unwrap();
    let trace = run(
        &session,
        "1. Call dump_rows exactly once.\n\
         2. Its output is long. Do not call dump_rows again. Find the line that starts with \
         NEEDLE_VALUE= using read or search on the full-output artifact the result names.\n\
         3. Answer with the full value after NEEDLE_VALUE=.",
    )
    .await;

    let dump = trace.named("dump_rows");
    assert_eq!(dump.len(), 1, "{trace:#?}");
    assert!(dump[0].output.contains("[tool output bounded by"));
    assert!(dump[0].output.contains("a3s://tool-output/dump_rows/"));
    assert!(!dump[0].output.contains("NEEDLE_VALUE"));
    assert!(trace.mentions("a3s://tool-output/dump_rows/"), "{trace:#?}");
    assert!(trace.text.contains("K7Q2-PLUM"), "{}", trace.text);

    let report = session.context_report().expect("context report");
    eprintln!("[harness-real] context report:\n{}", report.render());
    assert!(report.usage.total_tokens > 0);
    assert_eq!(report.tool_results, trace.calls.len());
    assert_eq!(report.heaviest[0].tool, "dump_rows");
    let cost = session.send("/cost", None).await.expect("/cost").text;
    eprintln!("[harness-real] /cost:\n{cost}");
    assert!(cost.contains(&format!("{} tool results", trace.calls.len())));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_sees_the_repeat_note_on_identical_polls() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(workspace.path(), "harness-real-repeat", &["poll_status(*)"]).await;
    session
        .register_dynamic_tool(Arc::new(PollStatus(AtomicUsize::new(0))))
        .unwrap();
    let trace = run(
        &session,
        "1. Call poll_status three times, one call per result.\n\
         2. Then answer REPEAT_NOTED if any result carried a host note that the output was identical, otherwise REPEAT_NOT_NOTED.",
    )
    .await;

    let polls = trace.named("poll_status");
    assert_eq!(polls.len(), 3, "{trace:#?}");
    assert!(!polls[0].output.contains("a3s host"));
    assert!(polls[1]
        .output
        .contains("identical arguments 2 times in a row"));
    assert!(polls[2]
        .output
        .contains("identical arguments 3 times in a row"));
    assert!(trace.text.contains("REPEAT_NOTED"), "{}", trace.text);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_searches_a_long_fetched_page_without_fetching_again() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(
        workspace.path(),
        "harness-real-web",
        &["web_fetch(*)", "read(*)", "search(*)"],
    )
    .await;
    let trace = run(
        &session,
        "1. Call web_fetch exactly once with url https://raw.githubusercontent.com/rust-lang/rust/master/RELEASES.md and no other arguments.\n\
         2. The page is long. Do not call web_fetch again. Use read or search on the full fetched text the result names to find the heading `Version 1.12.0 (2016-09-29)`.\n\
         3. Answer with the pull request number linked in the first bullet under that version's Highlights.",
    )
    .await;

    let fetch = trace.named("web_fetch");
    assert_eq!(fetch.len(), 1, "{trace:#?}");
    assert!(fetch[0]
        .output
        .contains("Full fetched text: a3s://tool-output/web_fetch/"));
    assert!(!fetch[0].output.contains("Version 1.12.0"));
    assert!(trace.mentions("a3s://tool-output/web_fetch/"), "{trace:#?}");
    assert!(trace.text.contains("34096"), "{}", trace.text);
}
