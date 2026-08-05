//! Real-LLM integration coverage for the context-efficient repository tools.
//!
//! Unlike the deterministic unit tests, these tests prove that a live model can
//! discover the schemas, send the new arguments, consume pagination metadata,
//! and preserve the promised filesystem side effects. Run them serially with:
//!
//! ```bash
//! scripts/context_tools_real_llm.sh
//! ```
//!
//! Set `A3S_CONTEXT_TOOLS_REAL_MODEL=provider/model` to override the config's
//! default model while keeping the same provider registry and credentials.
//! Set `A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN=1` to use the local Codex login.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, LlmClient, PlanningMode, SessionOptions,
    SystemPromptSlots,
};
use serde_json::{json, Value};
use support::codex_login_client::{default_codex_model, CodexLoginClient};

const REAL_LLM_TIMEOUT: Duration = Duration::from_secs(420);
const TEST_GUIDELINES: &str = "This is a deterministic integration test. Follow the user's numbered tool-call protocol exactly. Use only the tools visible to you, issue one requested call at a time, wait for its result before the next call, treat requested guard failures as expected evidence, and never skip, combine, retry, or improvise extra calls.";

#[derive(Debug)]
struct ToolCall {
    name: String,
    args: Value,
    output: String,
    exit_code: i32,
    metadata: Option<Value>,
    file_snapshot: Option<String>,
}

#[derive(Debug)]
struct Trace {
    calls: Vec<ToolCall>,
    final_text: String,
}

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn real_agent() -> Agent {
    let config = if use_codex_login() {
        CodeConfig::from_acl(
            r#"
            default_model = "openai/codex-login"
            providers "openai" {
              api_key = "test-only-overridden-client"
              models "codex-login" { name = "Codex Login" }
            }
            "#,
        )
        .expect("valid Codex-login test config")
    } else {
        let path = repo_config_path();
        CodeConfig::from_file(&path)
            .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()))
    };
    Agent::from_config(config)
        .await
        .expect("create agent from real provider config")
}

fn use_codex_login() -> bool {
    std::env::var("A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN").as_deref() == Ok("1")
}

fn restricted_options(rules: &[&str], max_tool_rounds: usize, session_id: &str) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(rules);
    policy.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id(session_id)
        .with_permission_policy(policy)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(max_tool_rounds)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(TEST_GUIDELINES));
    if use_codex_login() {
        let model = default_codex_model();
        eprintln!("[context-tools-real] Codex model={model} session={session_id}");
        let client: Arc<dyn LlmClient> = Arc::new(
            CodexLoginClient::from_local_login(&model, session_id)
                .expect("create client from local Codex login"),
        );
        return options.with_llm_client(client);
    }
    match std::env::var("A3S_CONTEXT_TOOLS_REAL_MODEL") {
        Ok(model) if !model.trim().is_empty() => options.with_model(model),
        _ => options,
    }
}

async fn run_prompt(session: &AgentSession, prompt: &str, snapshot_path: Option<&Path>) -> Trace {
    let (mut rx, handle) = session.stream(prompt, None).await.expect("start stream");
    let mut starts = HashMap::<String, (String, Value)>::new();
    let mut calls = Vec::new();

    let final_text = tokio::time::timeout(REAL_LLM_TIMEOUT, async {
        loop {
            let event = rx
                .recv()
                .await
                .expect("real-LLM stream closed before AgentEvent::End");
            match event {
                AgentEvent::ToolExecutionStart { id, name, args } => {
                    starts.insert(id, (name, args));
                }
                AgentEvent::ToolEnd {
                    id,
                    name,
                    args,
                    output,
                    exit_code,
                    metadata,
                    ..
                } => {
                    let (started_name, started_args) = starts
                        .remove(&id)
                        .unwrap_or_else(|| (name.clone(), args.unwrap_or_else(|| json!({}))));
                    assert_eq!(started_name, name, "tool start/end names diverged");
                    let file_snapshot = snapshot_path.map(|path| {
                        std::fs::read_to_string(path)
                            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
                    });
                    calls.push(ToolCall {
                        name,
                        args: started_args,
                        output,
                        exit_code,
                        metadata,
                        file_snapshot,
                    });
                }
                AgentEvent::End { text, .. } => break text,
                AgentEvent::Error { message } => panic!("real-LLM stream error: {message}"),
                AgentEvent::ConfirmationRequired {
                    tool_name, args, ..
                } => panic!("unexpected confirmation for {tool_name}: {args}"),
                _ => {}
            }
        }
    })
    .await
    .expect("real-LLM context-tool scenario timed out");

    handle.await.expect("stream worker joins");
    assert!(
        starts.is_empty(),
        "tool starts without matching ends: {starts:?}"
    );
    Trace { calls, final_text }
}

fn metadata(call: &ToolCall) -> &Value {
    call.metadata
        .as_ref()
        .unwrap_or_else(|| panic!("{} call omitted metadata: {call:?}", call.name))
}

fn output_mode(call: &ToolCall) -> &str {
    call.args["output_mode"]
        .as_str()
        .unwrap_or_else(|| panic!("grep call omitted output_mode: {call:?}"))
}

fn is_neutral_cursor(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(cursor)) => cursor.trim().is_empty(),
        Some(Value::Number(cursor)) => cursor.as_u64() == Some(0),
        Some(_) => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_batch_read_preserves_errors_and_follows_lossless_continuation() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::write(workspace.path().join("intro.txt"), "BATCH_HEAD_ALPHA\n").expect("write intro");
    let mut bulk = (0..40)
        .map(|index| format!("ROW-{index:03}-{}", "x".repeat(64)))
        .collect::<Vec<_>>()
        .join("\n");
    bulk.push_str("\nFINAL_MARKER=OMEGA_7391\n");
    std::fs::write(workspace.path().join("bulk.txt"), bulk).expect("write bulk");

    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(restricted_options(
                &["read(*)"],
                12,
                "context-tools-batch-read",
            )),
        )
        .await
        .expect("batch-read session");
    let prompt = r#"Use only the read tool and follow this protocol:
1. First call read with files in exactly this order: intro.txt, missing.txt, bulk.txt, and set max_output_bytes to 2048. Do not use file_path.
2. The missing member is intentional. After every result, inspect metadata.batch.continuation. While it is non-empty, call read again with files copied exactly from that continuation (and max_output_bytes 2048). Do not restart completed files.
3. When continuation is empty, report only the value found after FINAL_MARKER= in the file content. Do not guess it before reading the final page."#;
    let trace = run_prompt(&session, prompt, None).await;

    assert!(trace.calls.len() >= 2, "expected a continuation: {trace:?}");
    assert!(trace.calls.iter().all(|call| call.name == "read"));
    let first = &trace.calls[0];
    assert!(first.args.get("file_path").is_none());
    assert_eq!(first.args["max_output_bytes"], 2048);
    assert_eq!(
        first.args["files"],
        json!([
            {"path": "intro.txt"},
            {"path": "missing.txt"},
            {"path": "bulk.txt"}
        ])
    );
    assert!(first.output.contains("BATCH_HEAD_ALPHA"));
    assert!(first.output.contains("missing.txt") && first.output.contains("Failed to read"));
    assert_eq!(metadata(first)["batch"]["requested_files"], 3);
    assert_eq!(metadata(first)["batch"]["failed_files"], 1);
    assert_eq!(metadata(first)["batch"]["truncated"], true);

    for pair in trace.calls.windows(2) {
        let continuation = &metadata(&pair[0])["batch"]["continuation"];
        assert!(
            continuation
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "model made an extra read after completion: {pair:?}"
        );
        assert_eq!(
            pair[1].args["files"], *continuation,
            "model did not copy the lossless continuation"
        );
    }
    let last = trace.calls.last().expect("last read");
    assert_eq!(metadata(last)["batch"]["continuation"], json!([]));
    let all_output = trace
        .calls
        .iter()
        .map(|call| call.output.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(all_output.matches("BATCH_HEAD_ALPHA").count(), 1);
    assert_eq!(all_output.matches("FINAL_MARKER=OMEGA_7391").count(), 1);
    assert!(trace.final_text.contains("OMEGA_7391"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_grep_uses_all_output_modes_and_lexical_pages() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    for (name, body) in [
        ("zeta.txt", "NEEDLE zeta\n"),
        ("alpha.txt", "NEEDLE alpha one\nNEEDLE alpha two\n"),
        (
            "middle.txt",
            "NEEDLE middle one\nignore\nNEEDLE middle two\nNEEDLE middle three\n",
        ),
        ("none.txt", "nothing here\n"),
        ("excluded.md", "NEEDLE excluded\n"),
    ] {
        std::fs::write(workspace.path().join(name), body).expect("write grep fixture");
    }
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(restricted_options(
                &["search(*)"],
                12,
                "context-tools-grep-modes",
            )),
        )
        .await
        .expect("grep session");
    let prompt = r#"Use only search with mode='grep', query='NEEDLE', path '.', and include='*.txt'. Execute these steps in order, one call per result:
1. output_mode='content' with no limit or cursor.
2. output_mode='files_with_matches' with limit=2 and no cursor; if a cursor is returned, make exactly one continuation call with the same fields and that exact cursor.
3. output_mode='count' with limit=2 and no cursor; if a cursor is returned, make exactly one continuation call with the same fields and that exact cursor.
4. output_mode='summary' with no limit or cursor.
After all calls, end with the token GREP_SEQUENCE_OK. Do not use read or any other tool."#;
    let trace = run_prompt(&session, prompt, None).await;

    assert_eq!(trace.calls.len(), 6, "unexpected grep sequence: {trace:?}");
    assert!(trace.calls.iter().all(|call| call.name == "search"));
    let modes = trace.calls.iter().map(output_mode).collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            "content",
            "files_with_matches",
            "files_with_matches",
            "count",
            "count",
            "summary"
        ]
    );
    assert!(
        trace.calls.iter().all(|call| call.exit_code == 0),
        "a grep call failed: {trace:#?}"
    );
    assert!(trace.calls[0].output.contains("6 match(es) in 3 file(s)"));

    for first_index in [1usize, 3usize] {
        let first = &trace.calls[first_index];
        let second = &trace.calls[first_index + 1];
        assert!(is_neutral_cursor(first.args.get("cursor")));
        assert_eq!(first.args["limit"], 2);
        let cursor = &metadata(first)["page"]["next_cursor"];
        assert!(cursor.is_string(), "first page omitted cursor: {first:?}");
        assert_eq!(second.args["cursor"], *cursor);
        assert_eq!(metadata(second)["page"]["truncated"], false);
    }
    assert_eq!(
        metadata(&trace.calls[1])["source_anchors"],
        json!(["alpha.txt", "middle.txt"])
    );
    assert_eq!(
        metadata(&trace.calls[2])["source_anchors"],
        json!(["zeta.txt"])
    );
    assert!(trace.calls[3]
        .output
        .contains("alpha.txt: 2 matching line(s)"));
    assert!(trace.calls[3]
        .output
        .contains("middle.txt: 3 matching line(s)"));
    assert!(trace.calls[4]
        .output
        .contains("zeta.txt: 1 matching line(s)"));
    let summary = metadata(&trace.calls[5]);
    assert_eq!(summary["output_mode"], "summary");
    assert_eq!(summary["search"]["matching_lines"], 6);
    assert_eq!(summary["search"]["matching_files"], 3);
    assert_eq!(summary["search"]["truncated"], false);
    assert!(trace.final_text.contains("GREP_SEQUENCE_OK"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_edit_previews_rejects_guard_mismatches_then_commits() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    let path = workspace.path().join("edit.txt");
    let original = "first=OLD_TOKEN\nsecond=OLD_TOKEN\nthird=OLD_TOKEN\n";
    let edited = "first=NEW_TOKEN\nsecond=NEW_TOKEN\nthird=NEW_TOKEN\n";
    std::fs::write(&path, original).expect("write edit fixture");
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(restricted_options(
                &["edit(*)"],
                10,
                "context-tools-edit-guards",
            )),
        )
        .await
        .expect("edit session");
    let prompt = r#"Use only edit on edit.txt, replacing OLD_TOKEN with NEW_TOKEN and always set replace_all=true. Execute exactly these four calls in order, one per result:
1. dry_run=true, expected_replacements=3, max_replacements=3. This must preview without writing.
2. dry_run=false, expected_replacements=4, max_replacements=4. This count mismatch is intentional; accept the error and do not retry it.
3. dry_run=false, expected_replacements=3, max_replacements=2. This ceiling failure is intentional; accept the error and do not retry it.
4. dry_run=false, expected_replacements=3, max_replacements=3. This is the only call that should write.
After the fourth result, end with EDIT_SEQUENCE_OK and make no more tool calls."#;
    let trace = run_prompt(&session, prompt, Some(&path)).await;

    assert_eq!(trace.calls.len(), 4, "unexpected edit sequence: {trace:?}");
    assert!(trace.calls.iter().all(|call| call.name == "edit"));
    for call in &trace.calls {
        assert_eq!(call.args["file_path"], "edit.txt");
        assert_eq!(call.args["old_string"], "OLD_TOKEN");
        assert_eq!(call.args["new_string"], "NEW_TOKEN");
        assert_eq!(call.args["replace_all"], true);
    }

    let preview = &trace.calls[0];
    assert_eq!(preview.args["dry_run"], true);
    assert_eq!(preview.exit_code, 0);
    assert!(preview.output.contains("Dry run") && preview.output.contains("nothing was written"));
    assert_eq!(metadata(preview)["replacement_count"], 3);
    assert_eq!(preview.file_snapshot.as_deref(), Some(original));

    let expected_guard = &trace.calls[1];
    assert_eq!(expected_guard.args["expected_replacements"], 4);
    assert_ne!(expected_guard.exit_code, 0);
    assert!(expected_guard.output.contains("Replacement count mismatch"));
    assert_eq!(expected_guard.file_snapshot.as_deref(), Some(original));

    let maximum_guard = &trace.calls[2];
    assert_eq!(maximum_guard.args["expected_replacements"], 3);
    assert_eq!(maximum_guard.args["max_replacements"], 2);
    assert_ne!(maximum_guard.exit_code, 0);
    assert!(maximum_guard.output.contains("exceeds max_replacements=2"));
    assert_eq!(maximum_guard.file_snapshot.as_deref(), Some(original));

    let commit = &trace.calls[3];
    assert_eq!(commit.args["dry_run"], false);
    assert_eq!(commit.args["expected_replacements"], 3);
    assert_eq!(commit.args["max_replacements"], 3);
    assert_eq!(commit.exit_code, 0);
    assert_eq!(metadata(commit)["replacement_count"], 3);
    assert_eq!(commit.file_snapshot.as_deref(), Some(edited));
    assert_eq!(
        std::fs::read_to_string(path).expect("read final edit"),
        edited
    );
    assert!(trace.final_text.contains("EDIT_SEQUENCE_OK"));
}

fn item_paths(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.ends_with(".item"))
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_llm_glob_preserves_backend_default_and_uses_stable_path_pages() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    for name in ["zeta.item", "alpha.item", "middle.item", "beta.item"] {
        std::fs::write(workspace.path().join(name), name).expect("write glob fixture");
    }
    std::fs::write(workspace.path().join("ignore.txt"), "ignore").expect("write ignored file");
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(restricted_options(
                &["search(*)"],
                8,
                "context-tools-glob-sorting",
            )),
        )
        .await
        .expect("glob session");
    let prompt = r#"Use only search with mode='glob', query='*.item', and path '.'. Execute in order:
1. Call with limit=10 and OMIT the sort and cursor fields, so the backend default is exercised.
2. Call with sort='path', limit=2, and no cursor.
3. If step 2 returns a cursor, call again with sort='path', limit=2, and that exact cursor. Continue only until the cursor is absent.
Then end with GLOB_SEQUENCE_OK and make no extra calls."#;
    let trace = run_prompt(&session, prompt, None).await;

    assert_eq!(trace.calls.len(), 3, "unexpected glob sequence: {trace:?}");
    assert!(trace.calls.iter().all(|call| call.name == "search"));
    let backend = &trace.calls[0];
    assert!(
        backend.args.get("sort").is_none()
            || backend.args.get("sort").and_then(Value::as_str) == Some("backend")
    );
    assert!(is_neutral_cursor(backend.args.get("cursor")));
    assert_eq!(metadata(backend)["sort"], "backend");
    assert_eq!(item_paths(&backend.output).len(), 4);

    let first = &trace.calls[1];
    let second = &trace.calls[2];
    assert_eq!(first.args["sort"], "path");
    assert!(is_neutral_cursor(first.args.get("cursor")));
    assert_eq!(metadata(first)["sort"], "path");
    let cursor = &metadata(first)["page"]["next_cursor"];
    assert!(cursor.is_string());
    assert_eq!(second.args["cursor"], *cursor);
    assert_eq!(metadata(second)["page"]["truncated"], false);
    let paths = [item_paths(&first.output), item_paths(&second.output)].concat();
    assert_eq!(
        paths,
        ["alpha.item", "beta.item", "middle.item", "zeta.item"]
    );
    assert!(trace.final_text.contains("GLOB_SEQUENCE_OK"));
}
