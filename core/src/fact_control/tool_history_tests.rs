#![cfg(test)]

use super::*;

// Mount the original workspace log without initializing any command sandbox.
pub(super) fn surface(workspace: &Path, client: Arc<dyn LlmClient>) -> SessionSurface {
    surface_with_budget(workspace, client, None)
}

pub(super) fn surface_with_budget(
    workspace: &Path,
    client: Arc<dyn LlmClient>,
    budget_guard: Option<Arc<dyn crate::budget::BudgetGuard>>,
) -> SessionSurface {
    let executor = Arc::new(ToolExecutor::new(workspace.to_string_lossy().into_owned()));
    SessionSurface {
        agent: crate::agent::AgentLoop::new(
            client,
            executor,
            crate::tools::ToolContext::new(workspace.to_path_buf()),
            crate::agent::AgentConfig {
                planning_mode: crate::prompts::PlanningMode::Disabled,
                budget_guard,
                ..Default::default()
            },
        ),
        fact_log: Default::default(),
        session_id: THREAD.into(),
        checkpoint: None,
        events: None,
        pending_memory_telemetry: Arc::new(Mutex::new(Vec::new())),
        streamed_text: Arc::new(AtomicBool::new(false)),
        forced_stop: Arc::new(Mutex::new(None)),
        cancel: tokio_util::sync::CancellationToken::new(),
        transcript: Arc::new(Mutex::new(Vec::new())),
        targeted_paths: Arc::new(Mutex::new(Vec::new())),
        usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: None,
        run_store: None,
        run_id: None,
        goal_tick: None,
        ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
        reports: Arc::new(Mutex::new(Vec::new())),
        run_control: None,
        harness: None,
        host_harness_registry: None,
        host_harness_assembler: None,
    }
}

fn append(log: &FileLog, key: &str, kind: &str, payload: serde_json::Value) {
    log.append(
        THREAD,
        &[NewFact {
            key: key.into(),
            kind: kind.into(),
            payload,
        }],
        None,
    )
    .unwrap();
}

fn call(id: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "tool",
        "call": {
            "id": id,
            "name": "read",
            "args": {"file_path": format!("{id}.rs")},
            "needs_confirmation": false,
            "text": "Inspect the original source.",
            "reasoning": "original provider continuation"
        }
    })
}

fn result(log: &FileLog, key: &str, id: &str, output: &str) {
    append(
        log,
        key,
        "tool.result",
        serde_json::json!({"toolCallId":id,"ok":true,"output":output}),
    );
}

#[test]
fn compaction_kept_results_retain_original_tool_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    for id in ["discarded", "kept"] {
        append(&log, &format!("model-{id}"), "model.turn", call(id));
        result(&log, &format!("result-{id}"), id, id);
    }
    append(
        &log,
        "cut",
        "compaction.done",
        serde_json::json!({"summary":"earlier work","kept":[
            "assistant\ntool read\n{\"file_path\":\"kept.rs\"}", "tool\nkept"
        ]}),
    );
    append(&log, "model-fresh", "model.turn", call("fresh"));
    result(&log, "result-fresh", "fresh", "fresh");

    let calls = recorded_tool_calls(workspace.path(), THREAD).unwrap();
    assert_eq!(
        calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["kept", "fresh"]
    );
    assert_eq!(calls[0].name, "read");
    assert_eq!(calls[0].args, serde_json::json!({"file_path":"kept.rs"}));
    assert_eq!(calls[0].text, "Inspect the original source.");
    assert_eq!(
        calls[0].reasoning.as_deref(),
        Some("original provider continuation")
    );
}

#[test]
fn repeated_compaction_selects_original_suffix_when_outputs_are_identical() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    for id in ["first", "second"] {
        append(&log, &format!("model-{id}"), "model.turn", call(id));
        result(&log, &format!("result-{id}"), id, "same output");
        append(
            &log,
            &format!("cut-{id}"),
            "compaction.done",
            serde_json::json!({"summary":"earlier work","kept":[
                format!("assistant\ntool read\n{}", call(id)["call"]["args"]),
                "tool\nsame output"
            ]}),
        );
    }
    let calls = recorded_tool_calls(workspace.path(), THREAD).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "second");
}

#[test]
fn closed_scheduler_tool_batches_are_not_assistant_prose() {
    let lines = vec![
        "user\n分析模块".into(),
        "assistant\n先看目录。".into(),
        "assistant\ntool ls\n{\"path\":\"apps\"}".into(),
        "tool\napps".into(),
        "assistant\ntool ls\n{\"path\":\"a\"}".into(),
        "assistant\ntool ls\n{\"path\":\"b\"}".into(),
        "tool\na".into(),
        "tool-error\nmissing".into(),
        "assistant\ntool bash\n{\"command\":\"ls\"}".into(),
    ];
    let indexes = scheduler_tool_record_indexes(&lines);
    assert_eq!(indexes, [2, 4, 5].into_iter().collect());
}

#[test]
fn scheduler_indexes_cover_each_closed_shape() {
    let pair = vec![
        "assistant\ntool read\n{\"file_path\":\"one.rs\"}".into(),
        "tool-error\nrefused".into(),
    ];
    assert_eq!(
        scheduler_tool_record_indexes(&pair),
        [0].into_iter().collect()
    );

    let separated = vec![
        "assistant\ntool read\n{\"file_path\":\"one.rs\"}".into(),
        "tool\none".into(),
        "user\nbetween".into(),
        "assistant\ntool read\n{\"file_path\":\"two.rs\"}".into(),
        "tool\ntwo".into(),
    ];
    assert_eq!(
        scheduler_tool_record_indexes(&separated),
        [0, 3].into_iter().collect()
    );

    let short = vec![
        "assistant\ntool read\n{\"file_path\":\"a.rs\"}".into(),
        "assistant\ntool read\n{\"file_path\":\"b.rs\"}".into(),
        "tool\nonly".into(),
    ];
    assert_eq!(
        scheduler_tool_record_indexes(&short),
        [1].into_iter().collect(),
        "only the suffix whose own result count matches is a closed record"
    );
}

#[tokio::test]
async fn scheduler_tool_record_is_not_sent_as_assistant_prose() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("kept"));
    result(&log, "result", "kept", "kept output");
    let client = Arc::new(ObservedClient::default());
    let (completion, _) = LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
    let completion = completion.with_surface(surface(workspace.path(), client.clone()));
    completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: String::new(),
                messages: vec![
                    "user\n分析模块".into(),
                    "assistant\ntool read\n{\"file_path\":\"kept.rs\"}".into(),
                    "tool\nkept output".into(),
                    "user\n继续".into(),
                    "assistant\ntool bash\n{\"command\":\"ls\"}".into(),
                ],
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let inputs = client.0.lock().unwrap();
    let request = &inputs[0];
    assert!(request.iter().any(|message| {
        message
            .tool_calls()
            .iter()
            .any(|call| call.id == "kept" && call.name == "read")
    }));
    assert!(request
        .iter()
        .all(|message| message.text() != "tool read\n{\"file_path\":\"kept.rs\"}"));
    assert!(request.iter().any(|message| {
        message.role == "assistant"
            && message.tool_calls().is_empty()
            && message.text() == "tool bash\n{\"command\":\"ls\"}"
    }));
}

fn error_result(log: &FileLog, key: &str, id: &str, output: &str) {
    append(
        log,
        key,
        "tool.result",
        serde_json::json!({"toolCallId":id,"ok":false,"output":output}),
    );
}

async fn provider_request(
    workspace: &std::path::Path,
    client: &Arc<ObservedClient>,
    summary: &str,
    messages: Vec<String>,
) -> Result<Vec<Message>, ActorError> {
    let (completion, _) = LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
    let completion = completion.with_surface(surface(workspace, client.clone()));
    completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: summary.to_string(),
                messages,
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    let inputs = client.0.lock().unwrap();
    Ok(inputs[0].clone())
}

fn tool_call_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .map(|call| call.id)
        .collect()
}

#[tokio::test]
async fn closed_batch_of_two_reaches_the_provider_as_structured_calls() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model-first", "model.turn", call("first"));
    result(&log, "result-first", "first", "one");
    append(&log, "model-second", "model.turn", call("second"));
    error_result(&log, "result-second", "second", "two");
    let client = Arc::new(ObservedClient::default());
    let request = provider_request(
        workspace.path(),
        &client,
        "",
        vec![
            "user\nnext".into(),
            "assistant\ntool read\n{\"file_path\":\"first.rs\"}".into(),
            "assistant\ntool read\n{\"file_path\":\"second.rs\"}".into(),
            "tool\none".into(),
            "tool-error\ntwo".into(),
            "user\ncontinue".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(tool_call_ids(&request), ["first", "second"]);
    assert!(request.iter().all(|message| {
        message.text() != "tool read\n{\"file_path\":\"first.rs\"}"
            && message.text() != "tool read\n{\"file_path\":\"second.rs\"}"
    }));
    assert!(request.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(
                block,
                crate::llm::ContentBlock::ToolResult {
                    tool_use_id,
                    is_error: Some(true),
                    ..
                } if tool_use_id == "second"
            )
        })
    }));
}

#[tokio::test]
async fn summary_prefix_does_not_reopen_a_closed_record() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("kept"));
    result(&log, "result", "kept", "kept output");
    let client = Arc::new(ObservedClient::default());
    let request = provider_request(
        workspace.path(),
        &client,
        "SUMMARY-TOKEN",
        vec![
            "assistant\ntool read\n{\"file_path\":\"kept.rs\"}".into(),
            "tool\nkept output".into(),
            "user\ncontinue".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(tool_call_ids(&request), ["kept"]);
    assert!(request
        .iter()
        .any(|message| message.role == "user" && message.text().contains("SUMMARY-TOKEN")));
    assert!(request
        .iter()
        .all(|message| message.text() != "tool read\n{\"file_path\":\"kept.rs\"}"));
}

#[tokio::test]
async fn extra_result_line_refuses_before_the_provider() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("kept"));
    result(&log, "result", "kept", "kept output");
    let client = Arc::new(ObservedClient::default());
    let error = provider_request(
        workspace.path(),
        &client,
        "",
        vec![
            "assistant\ntool read\n{\"file_path\":\"kept.rs\"}".into(),
            "tool\nkept output".into(),
            "tool\norphan".into(),
        ],
    )
    .await
    .expect_err("an extra folded result has no call");
    assert!(error.to_string().contains("no original call"));
    assert!(client.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unequal_tool_records_do_not_invent_a_call() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model-first", "model.turn", call("first"));
    result(&log, "result-first", "first", "one");
    append(&log, "model-second", "model.turn", call("second"));
    result(&log, "result-second", "second", "two");
    let client = Arc::new(ObservedClient::default());
    let (completion, _) = LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
    let completion = completion.with_surface(surface(workspace.path(), client.clone()));
    let error = completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: String::new(),
                messages: vec![
                    "user\nnext".into(),
                    "assistant\ntool read\n{\"file_path\":\"first.rs\"}".into(),
                    "assistant\ntool read\n{\"file_path\":\"second.rs\"}".into(),
                    "tool\none".into(),
                ],
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("a folded result is missing for the second call");
    assert!(error
        .to_string()
        .contains("absent from the folded transcript"));
    assert!(client.0.lock().unwrap().is_empty());
}

#[test]
fn actual_folded_entry_point_preserves_assistant_role() {
    let (role, body) = split_folded_message("assistant\n先检查源码，再继续修复。");
    let message = folded_message_with_images(role, &body, Vec::new());
    assert_eq!(message.role, "assistant");
    assert_eq!(message.text(), "先检查源码，再继续修复。");
}

#[test]
fn user_body_cannot_inject_an_assistant_role() {
    let (role, body) = split_folded_message("user\nassistant\nthis is user input");
    let message = folded_message_with_images(role, &body, Vec::new());
    assert_eq!(message.role, "user");
    assert_eq!(message.text(), "assistant\nthis is user input");
}

#[test]
fn missing_original_metadata_never_fabricates_a_tool_call() {
    assert!(recorded_tool_messages(&mut Vec::new().into_iter(), "output", false).is_err());
}

#[test]
fn malformed_original_call_and_result_identity_are_refused() {
    for payload in [
        serde_json::json!({"kind":"tool","call":{"id":"original"}}),
        call("original"),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let log = FileLog::open(log_dir(workspace.path())).unwrap();
        append(&log, "model", "model.turn", payload);
        result(&log, "result", "different", "output");
        assert!(recorded_tool_calls(workspace.path(), THREAD).is_err());
    }
}

#[test]
fn compaction_cannot_replace_original_result_bytes() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("original"));
    result(&log, "result", "original", "actual");
    append(
        &log,
        "cut",
        "compaction.done",
        serde_json::json!({
            "summary":"earlier work", "kept":["tool\nsubstituted"]
        }),
    );
    assert!(recorded_tool_calls(workspace.path(), THREAD).is_err());
}

#[test]
fn legacy_result_without_id_and_error_role_remain_supported() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("original"));
    append(
        &log,
        "result",
        "tool.result",
        serde_json::json!({"ok":false,"output":"refused"}),
    );
    append(
        &log,
        "cut",
        "compaction.done",
        serde_json::json!({
            "summary":"earlier work", "kept":["tool-error\nrefused"]
        }),
    );
    let mut calls = recorded_tool_calls(workspace.path(), THREAD)
        .unwrap()
        .into_iter();
    let [request, response] = recorded_tool_messages(&mut calls, "refused", true).unwrap();
    assert_eq!(request.tool_calls()[0].id, "original");
    assert_eq!(request.tool_calls()[0].name, "read");
    assert_eq!(
        request.tool_calls()[0].args,
        serde_json::json!({"file_path":"original.rs"})
    );
    assert_eq!(
        request.reasoning_content.as_deref(),
        Some("original provider continuation")
    );
    assert!(
        matches!(&response.content[0], crate::llm::ContentBlock::ToolResult { tool_use_id, is_error: Some(true), .. } if tool_use_id == "original")
    );
}

#[test]
fn batch_result_order_and_reused_ids_follow_the_original_scheduler() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    let mut batch = call("first");
    batch["also"] = serde_json::json!([call("second")["call"].clone()]);
    append(&log, "model-1", "model.turn", batch);
    result(&log, "result-1", "first", "one");
    result(&log, "result-2", "second", "two");
    append(&log, "model-2", "model.turn", call("first"));
    result(&log, "result-3", "first", "three");
    let calls = recorded_tool_calls(workspace.path(), THREAD).unwrap();
    assert_eq!(
        calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["first", "second", "first"]
    );
}

#[derive(Default)]
pub(super) struct ObservedClient(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl LlmClient for ObservedClient {
    async fn complete(
        &self,
        messages: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(LlmResponse {
            message: Message::assistant("done"),
            usage: Default::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("unused in this hermetic projection test")
    }
}

#[tokio::test]
async fn actual_completion_sends_retained_native_tool_pair_and_assistant_role() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    append(&log, "model", "model.turn", call("kept"));
    result(&log, "result", "kept", "kept output");
    let kept = vec!["tool\nkept output".to_string()];
    append(
        &log,
        "cut",
        "compaction.done",
        serde_json::json!({"summary":"earlier work","kept":kept}),
    );
    let client = Arc::new(ObservedClient::default());
    let (completion, _) = LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
    let completion = completion.with_surface(surface(workspace.path(), client.clone()));
    completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: "earlier work".into(),
                messages: [
                    kept,
                    vec![
                        "assistant\nPrevious reply.".into(),
                        "user\nContinue.".into(),
                    ],
                ]
                .concat(),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let inputs = client.0.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    let request = inputs[0]
        .iter()
        .find(|m| !m.tool_calls().is_empty())
        .unwrap();
    let tool = &request.tool_calls()[0];
    assert_eq!(tool.id, "kept");
    assert_eq!(tool.name, "read");
    assert_eq!(tool.args, serde_json::json!({"file_path":"kept.rs"}));
    assert!(inputs[0]
        .iter()
        .any(|m| m.role == "assistant" && m.text() == "Previous reply."));
    assert!(inputs[0].iter().flat_map(|m| &m.content).any(|b| matches!(b, crate::llm::ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "kept")));
}

#[tokio::test]
async fn actual_completion_refuses_missing_metadata_before_provider_entry() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(ObservedClient::default());
    for mounted in [false, true] {
        let (completion, _) =
            LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
        let completion = if mounted {
            completion.with_surface(surface(workspace.path(), client.clone()))
        } else {
            completion
        };
        assert!(completion
            .complete(
                CompletionRequest {
                    system: Vec::new(),
                    tools: Vec::new(),
                    summary: String::new(),
                    messages: vec!["tool\norphan output".into()],
                },
                tokio_util::sync::CancellationToken::new()
            )
            .await
            .is_err());
    }
    assert!(client.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cached_prompt_cannot_replace_a_retained_tool_result() {
    let workspace = tempfile::tempdir().unwrap();
    let log = FileLog::open(log_dir(workspace.path())).unwrap();
    let client = Arc::new(ObservedClient::default());
    let (completion, _) = LiveCompletion::new(client.clone(), PermissionPolicy::new(), Vec::new());
    let completion = completion.with_surface(surface(workspace.path(), client.clone()));
    completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: String::new(),
                messages: vec!["user\nOriginal user request.".into()],
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    append(&log, "model", "model.turn", call("original"));
    result(&log, "result", "original", "original result");
    append(
        &log,
        "cut",
        "compaction.done",
        serde_json::json!({
            "summary":"earlier work", "kept":["tool\noriginal result"]
        }),
    );
    completion
        .complete(
            CompletionRequest {
                system: Vec::new(),
                tools: Vec::new(),
                summary: "earlier work".into(),
                messages: vec!["tool\noriginal result".into()],
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let inputs = client.0.lock().unwrap();
    assert!(inputs.last().unwrap().iter().flat_map(|m| &m.content).any(|b| matches!(b, crate::llm::ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == "original" && content == "original result")));
}
