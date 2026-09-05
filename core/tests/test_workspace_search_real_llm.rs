//! Live model qualification for the framework-owned workspace search path.
//!
//! This test deliberately does not prescribe a search mode in the request. The
//! model receives one unified `search` tool and must choose the mode from the
//! tool description. A natural-language repository question should select
//! `bm25`, which the default local workspace transparently serves from the
//! durable zvec FTS projection.
//!
//! Run through `scripts/workspace_search_real_llm.sh`; it is ignored because
//! it spends one real provider request and requires the configured ACL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, CodeConfig, PlanningMode, SessionOptions, SystemPromptSlots,
};
use serde_json::Value;

const INDEX_READY_TIMEOUT: Duration = Duration::from_secs(30);
const TURN_TIMEOUT: Duration = Duration::from_secs(180);
const EXPECTED_FUNCTION: &str = "suppress_replayed_envelopes";

fn config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

fn configured_model(config: &CodeConfig) -> String {
    let model = std::env::var("A3S_TEST_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config.default_model.clone())
        .expect("config must declare default_model or A3S_TEST_MODEL");
    let (provider, model_id) = model
        .split_once('/')
        .expect("selected model must use provider/model syntax");
    assert!(
        config.llm_config(provider, model_id).is_some(),
        "selected model {model} is not declared in the ACL"
    );
    model
}

fn write_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("src")).expect("create fixture source directory");
    std::fs::create_dir_all(root.join("docs")).expect("create fixture docs directory");
    std::fs::write(
        root.join("src/replay_fence.rs"),
        r#"//! Transport reconnect handling.
//!
//! A reconnect can deliver an envelope that was already accepted. The fence
//! records the accepted delivery id and suppresses the duplicate before it
//! reaches the application dispatcher.
pub fn suppress_replayed_envelopes(delivery_id: &str, last_accepted: &str) -> bool { // Prevents duplicate event delivery after a transport reconnect.
    delivery_id == last_accepted
}
"#,
    )
    .expect("write expected fixture source");
    std::fs::write(
        root.join("src/near_miss.rs"),
        r#"pub fn retry_transport_after_disconnect() -> bool { true }
"#,
    )
    .expect("write lexical decoy source");
    std::fs::write(
        root.join("docs/reconnect.md"),
        "Reconnect handling must prevent duplicate delivery after a transport reconnect.\n",
    )
    .expect("write documentation decoy");
}

async fn wait_for_native_index(root: &Path) {
    let current = root.join(".a3s-code/index/CURRENT");
    tokio::time::timeout(INDEX_READY_TIMEOUT, async {
        loop {
            if tokio::fs::try_exists(&current).await.unwrap_or(false) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "native zvec index did not become ready: {}",
            current.display()
        )
    });
}

#[derive(Debug)]
struct SearchCall {
    args: Value,
    metadata: Value,
    exit_code: i32,
}

async fn run_search_turn(
    session: &a3s_code_core::AgentSession,
) -> (String, SearchCall, usize, u128) {
    let prompt = r#"Investigate this workspace and answer the user's intent.

User intent: find the function that prevents duplicate event delivery after a transport reconnect.

Use exactly one call to the `search` tool and no other tool. Choose the search mode yourself from the schema: use the mode that fits this natural-language relevance question. Set `path` to `.`, `include` to `*.rs`, `limit` to `5`, and `context` to `2`. After the tool result, reply with exactly the function identifier supported by the returned evidence and nothing else."#;
    let started = Instant::now();
    let (mut events, worker) = session
        .stream(prompt, None)
        .await
        .expect("start model turn");
    let mut starts = HashMap::<String, (String, Value)>::new();
    let mut tool_names = Vec::new();
    let mut search_calls = Vec::new();
    let mut final_text = String::new();
    let usage = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events
                .recv()
                .await
                .expect("model event stream ended before completion")
            {
                AgentEvent::ToolExecutionStart { id, name, args } => {
                    starts.insert(id, (name, args));
                }
                AgentEvent::ToolEnd {
                    id,
                    name,
                    args,
                    metadata,
                    exit_code,
                    ..
                } => {
                    let (started_name, started_args) = starts
                        .remove(&id)
                        .unwrap_or_else(|| (name.clone(), args.unwrap_or_default()));
                    assert_eq!(started_name, name, "tool start/end names diverged");
                    tool_names.push(name.clone());
                    if name == "search" {
                        search_calls.push(SearchCall {
                            args: started_args,
                            metadata: metadata.unwrap_or(Value::Null),
                            exit_code,
                        });
                    }
                }
                AgentEvent::End { text, usage, .. } => {
                    final_text = text;
                    break usage.total_tokens;
                }
                AgentEvent::Error { message } => panic!("model turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("model search turn timed out");
    worker.await.expect("model stream worker joins");
    assert!(starts.is_empty(), "tool starts without terminal events");
    assert_eq!(
        tool_names,
        ["search"],
        "the model must use exactly one search tool call"
    );
    assert_eq!(search_calls.len(), 1, "expected exactly one search call");
    let call = search_calls.pop().expect("search call should be recorded");
    (final_text, call, usage, started.elapsed().as_millis())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_selects_bm25_and_uses_native_workspace_index() {
    let config_file = config_path();
    let config = CodeConfig::from_file(&config_file).expect("load .a3s/config.acl");
    let selected_model = configured_model(&config);
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let workspace = tempfile::tempdir().expect("create fixture workspace");
    write_fixture(workspace.path());
    let mut permissions = PermissionPolicy::new().allow_all(&["search(*)"]);
    permissions.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id("workspace-search-real-llm")
        .with_model(selected_model.clone())
        .with_permission_policy(permissions)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(2)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "Use only the requested one-tool protocol. Do not call read, glob, grep, or any other tool directly.",
        ));
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create workspace session");

    wait_for_native_index(workspace.path()).await;
    let (final_text, call, total_tokens, turn_ms) = run_search_turn(&session).await;
    assert_eq!(call.exit_code, 0, "search must succeed");
    assert_eq!(
        call.args["mode"], "bm25",
        "model should select ranked lexical search for a relevance question"
    );
    assert_eq!(call.args["path"], ".");
    assert_eq!(call.args["include"], "*.rs");
    assert_eq!(call.args["limit"], 5);
    assert_eq!(call.args["context"], 2);
    assert_eq!(call.metadata["mode"], "bm25");
    assert_eq!(call.metadata["execution_mode"], "persistent_zvec_fts");
    assert_eq!(call.metadata["index_kind"], "persistent_zvec_fts");
    assert_eq!(call.metadata["source_verified"], true);
    let paths = call.metadata["results"]
        .as_array()
        .expect("search metadata should contain results")
        .iter()
        .filter_map(|result| result["path"].as_str())
        .collect::<Vec<_>>();
    assert!(
        paths.contains(&"src/replay_fence.rs"),
        "native index results should contain the expected source: {paths:?}"
    );
    assert!(
        final_text.contains(EXPECTED_FUNCTION),
        "model answer must be grounded in the search result: {final_text:?}"
    );
    println!(
        "workspace-search-real-llm model={} mode=bm25 index=persistent_zvec_fts tokens={} turn_ms={} result=pass",
        selected_model, total_tokens, turn_ms
    );
    session.close().await;
}
