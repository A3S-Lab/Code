use std::collections::HashMap;
use std::sync::Arc;

use super::support::*;
use super::*;
use a3s_code_telemetry::config::TelemetryMode;
use a3s_code_tools::implementations::grok_build::grep::GrepTool;
use a3s_code_tools::implementations::grok_build::read_file::ReadFileTool;
use a3s_code_tools::implementations::grok_build::search_replace::SearchReplaceTool;
use a3s_code_tools::implementations::opencode::OpenCodeWriteTool;
use a3s_code_tools::registry::types::ToolConfig;
use serde_json::Value;
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

fn search_call(id: &str, path: &str) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            "search_code",
            serde_json::json!({"pattern": "needle-keep", "path": path}).to_string(),
        ),
    }
}

async fn tool_result_text(actor: &SessionActor, call_id: &str) -> String {
    let conv = actor.chat_state_handle.get_conversation().await;
    conv.iter()
        .rev()
        .find_map(|item| match item {
            a3s_code_sampling_types::ConversationItem::ToolResult(result)
                if result.tool_call_id == call_id =>
            {
                Some(result.content.to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_result for {call_id}"))
}

#[serial_test::serial(tool_call_telemetry)]
#[tokio::test(flavor = "current_thread")]
async fn renamed_grep_keeps_its_output_after_a_later_model_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = std::env::temp_dir().join(format!("grep-model-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("note.txt");
            std::fs::write(&file, "needle-keep\n").unwrap();
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![
                ToolConfig::for_tool::<GrepTool>().with_name("search_code"),
            ])
            .await;
            let toolset = actor.agent.borrow().tool_bridge().toolset();
            let (id, version) = crate::session::telemetry::tool_identity(&toolset, "search_code");
            assert_eq!(id, "GrokBuild:grep");
            assert_eq!(version.as_deref(), Some("current"));
            let (unknown, unknown_version) =
                crate::session::telemetry::tool_identity(&toolset, "not_a_tool");
            assert_eq!(unknown, "opaque");
            assert_eq!(unknown_version, None);
            actor
                .workspace_ops
                .bind_local_session(
                    &actor.session_id_string(),
                    actor.tool_context.cwd.as_path().to_path_buf(),
                    actor.tool_context.hunk_tracker_handle.clone(),
                    toolset.clone(),
                    None,
                )
                .expect("bind_local_session");

            let mut deferred = Vec::new();
            let prepared = actor
                .prepare_tool_call(
                    search_call("prep", file.to_str().unwrap()),
                    &mut deferred,
                    Some("grok-4.6"),
                )
                .await
                .expect("prepare")
                .expect("search_code prepares");
            assert_eq!(prepared.tool_id, "GrokBuild:grep");
            assert_eq!(prepared.model_id.as_deref(), Some("grok-4.6"));
            assert_ne!(prepared.invocation_id, prepared.call_id);
            let mut config = actor
                .chat_state_handle
                .get_sampling_config()
                .await
                .expect("sampling config");
            config.model = "grok-4.5".into();
            actor
                .chat_state_handle
                .update_sampling_config(config.clone());
            assert_eq!(prepared.model_id.as_deref(), Some("grok-4.6"));

            actor
                .execute_tool_calls(
                    vec![search_call("grep-1", file.to_str().unwrap())],
                    Some("grok-4.6".into()),
                )
                .await
                .expect("execute");
            let direct = toolset
                .call(
                    "search_code",
                    serde_json::json!({"pattern": "needle-keep", "path": file.to_str().unwrap()}),
                    "direct",
                    None,
                )
                .await
                .expect("direct grep");
            let stored = tool_result_text(&actor, "grep-1").await;
            assert_eq!(stored, direct.prompt_text);
            let request = actor
                .chat_state_handle
                .build_request(Vec::new(), None, false, None, "conv".into(), "req".into())
                .await
                .expect("later request");
            assert_eq!(request.model.as_deref(), Some("grok-4.5"));
            let later = request
                .items
                .iter()
                .find_map(|item| match item {
                    a3s_code_sampling_types::ConversationItem::ToolResult(result)
                        if result.tool_call_id == "grep-1" =>
                    {
                        Some(result.content.to_string())
                    }
                    _ => None,
                })
                .expect("tool result in later request");
            assert_eq!(later, direct.prompt_text);
            let _ = std::fs::remove_dir_all(dir);
        })
        .await;
}

const MODEL: &str = "grok-tool-telemetry";

struct ResetTelemetry;

impl Drop for ResetTelemetry {
    fn drop(&mut self) {
        let config = a3s_code_telemetry::config::TelemetryConfig {
            mixpanel_enabled: false,
            mixpanel_token: None,
            ..a3s_code_telemetry::config::TelemetryConfig::default()
        };
        a3s_code_telemetry::init(
            config,
            TelemetryMode::Disabled,
            None,
            None,
            None,
            None,
            "test".into(),
            None,
            crate::http::shared_client(),
        );
    }
}

fn init_product(server: &a3s_code_test_support::MockInferenceServer, mode: TelemetryMode) {
    let config = a3s_code_telemetry::config::TelemetryConfig {
        events_url: Some(format!("{}/events", server.url())),
        events_api_key: Some("test-key".into()),
        mixpanel_enabled: false,
        mixpanel_token: None,
        ..a3s_code_telemetry::config::TelemetryConfig::default()
    };
    // `shared_client` keeps idle sockets process-wide. `TcpListener::bind` can
    // recycle a loopback port onto a dead connection, and `track` drops that error.
    let client = reqwest::Client::builder()
        .http1_only()
        .pool_max_idle_per_host(0)
        .build()
        .expect("telemetry test client");
    a3s_code_telemetry::init(
        config,
        mode,
        None,
        None,
        None,
        None,
        "test".into(),
        None,
        client,
    );
}

fn tool_call(
    id: &str,
    name: &str,
    args: serde_json::Value,
) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(name, args.to_string()),
    }
}

#[derive(Clone)]
struct CapturedSpan {
    fields: HashMap<String, String>,
}

struct CaptureLayer {
    spans: Arc<parking_lot::Mutex<Vec<CapturedSpan>>>,
}

struct FieldVisitor<'a>(&'a mut HashMap<String, String>);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let mut fields = HashMap::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        let Some(fields) = extensions.get_mut::<HashMap<String, String>>() else {
            return;
        };
        values.record(&mut FieldVisitor(fields));
    }

    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        if span.name() != "tool.execution" {
            return;
        }
        let fields = span
            .extensions()
            .get::<HashMap<String, String>>()
            .cloned()
            .unwrap_or_default();
        self.spans.lock().push(CapturedSpan { fields });
    }
}

fn field<'a>(span: &'a CapturedSpan, key: &str) -> Option<&'a str> {
    span.fields.get(key).map(|value| value.trim_matches('"'))
}

fn span_for<'a>(spans: &'a [CapturedSpan], call_id: &str) -> &'a CapturedSpan {
    spans
        .iter()
        .find(|span| field(span, "tool_call_id") == Some(call_id))
        .unwrap_or_else(|| panic!("no tool.execution for {call_id}"))
}

fn product_event<'a>(rows: &'a [Value], invocation_id: &str) -> &'a Value {
    rows.iter()
        .find(|event| {
            event.get("event_name").and_then(Value::as_str)
                == Some("grok-shell-tool_call_completed")
                && event
                    .get("event_metadata")
                    .and_then(|metadata| metadata.get("invocation_id"))
                    .and_then(Value::as_str)
                    == Some(invocation_id)
        })
        .unwrap_or_else(|| panic!("no product row for {invocation_id}"))
}

fn row_metadata<'a>(rows: &'a [Value], invocation_id: &str) -> &'a serde_json::Map<String, Value> {
    product_event(rows, invocation_id)
        .get("event_metadata")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("no metadata for {invocation_id}"))
}

fn meta_str<'a>(metadata: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(Value::as_str)
}

fn assert_grep(
    spans: &[CapturedSpan],
    rows: &[Value],
    call_id: &str,
    status: &str,
    reason: Option<&str>,
    outcome: &str,
) {
    let span = span_for(spans, call_id);
    let invocation = field(span, "invocation_id").unwrap_or_else(|| panic!("{call_id} invocation"));
    let event = product_event(rows, invocation);
    let metadata = row_metadata(rows, invocation);
    assert_eq!(field(span, "model_id"), Some(MODEL), "{call_id}");
    assert_eq!(meta_str(metadata, "model_id"), Some(MODEL), "{call_id}");
    assert_eq!(
        meta_str(metadata, "invocation_id"),
        Some(invocation),
        "{call_id}"
    );
    assert_eq!(field(span, "tool_id"), Some("GrokBuild:grep"), "{call_id}");
    assert_eq!(
        meta_str(metadata, "tool_id"),
        Some("GrokBuild:grep"),
        "{call_id}"
    );
    assert_eq!(field(span, "tool_version"), Some("current"), "{call_id}");
    assert_eq!(
        meta_str(metadata, "tool_version"),
        Some("current"),
        "{call_id}"
    );
    assert_eq!(field(span, "source_status"), Some(status), "{call_id}");
    assert_eq!(
        meta_str(metadata, "source_status"),
        Some(status),
        "{call_id}"
    );
    assert_eq!(field(span, "source_reason"), reason, "{call_id}");
    assert_eq!(meta_str(metadata, "source_reason"), reason, "{call_id}");
    assert_eq!(field(span, "outcome"), Some(outcome), "{call_id}");
    assert!(field(span, "read_file_role").is_none(), "{call_id}");
    assert!(metadata.get("read_file_role").is_none(), "{call_id}");
    assert_eq!(meta_str(metadata, "outcome"), Some(outcome), "{call_id}");
    assert_eq!(
        field(span, "success"),
        Some(if outcome == "error" { "false" } else { "true" }),
        "{call_id}"
    );
    assert_eq!(
        meta_str(metadata, "tool_name"),
        Some("search_code"),
        "{call_id}"
    );
    assert!(field(span, "session_id").is_some(), "{call_id}");
    assert!(!span.fields.contains_key("path_scope"), "{call_id}");
    assert!(!metadata.contains_key("path_scope"), "{call_id}");
    assert!(!event.to_string().contains("secret-project"), "{call_id}");
}

fn assert_tmp(
    spans: &[CapturedSpan],
    rows: &[Value],
    call_id: &str,
    tool_name: &str,
    tool_id: &str,
) {
    let span = span_for(spans, call_id);
    let invocation = field(span, "invocation_id").unwrap_or_else(|| panic!("{call_id} invocation"));
    let event = product_event(rows, invocation);
    let metadata = row_metadata(rows, invocation);
    assert_eq!(meta_str(metadata, "path_scope"), Some("tmp"), "{call_id}");
    assert_eq!(
        meta_str(metadata, "tool_name"),
        Some(tool_name),
        "{call_id}"
    );
    assert_eq!(field(span, "tool_id"), Some(tool_id), "{call_id}");
    assert_eq!(meta_str(metadata, "tool_id"), Some(tool_id), "{call_id}");
    assert_eq!(field(span, "model_id"), Some(MODEL), "{call_id}");
    assert_eq!(meta_str(metadata, "model_id"), Some(MODEL), "{call_id}");
    assert_eq!(
        meta_str(metadata, "invocation_id"),
        Some(invocation),
        "{call_id}"
    );
    assert!(!span.fields.contains_key("path_scope"), "{call_id}");
    assert!(
        !format!("{:?}", span.fields).contains("secret-project"),
        "{call_id}"
    );
    assert!(!event.to_string().contains("secret-project"), "{call_id}");
    assert!(!event.to_string().contains("CANARY_BODY"), "{call_id}");
}

#[serial_test::serial(tool_call_telemetry)]
#[tokio::test(flavor = "current_thread")]
async fn execute_tool_calls_records_the_product_row_and_execution_span() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _reset = ResetTelemetry;
            let product = a3s_code_test_support::MockInferenceServer::start()
                .await
                .expect("product");
            init_product(&product, TelemetryMode::SessionMetrics);
            let scratch =
                std::env::temp_dir().join(format!("secret-project-{}", std::process::id()));
            let repo = std::env::temp_dir().join(format!("opt-repo-{}", std::process::id()));
            std::fs::create_dir_all(&scratch).unwrap();
            std::fs::create_dir_all(&repo).unwrap();
            let note = scratch.join("note.txt");
            let out = scratch.join("out.txt");
            std::fs::write(&note, "needle-keep\nalpha\n").unwrap();
            let note_path = note.to_string_lossy().into_owned();
            let out_path = out.to_string_lossy().into_owned();
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![
                ToolConfig::for_tool::<GrepTool>().with_name("search_code"),
                ToolConfig::for_tool::<ReadFileTool>().with_name("read_note"),
                ToolConfig::for_tool::<SearchReplaceTool>(),
                ToolConfig::for_tool::<OpenCodeWriteTool>(),
            ])
            .await;
            let toolset = actor.agent.borrow().tool_bridge().toolset();
            actor
                .workspace_ops
                .bind_local_session(
                    &actor.session_id_string(),
                    actor.tool_context.cwd.as_path().to_path_buf(),
                    actor.tool_context.hunk_tracker_handle.clone(),
                    toolset,
                    None,
                )
                .expect("bind_local_session");
            actor.session_info.cwd = repo.to_string_lossy().into_owned();

            let spans = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::registry().with(CaptureLayer {
                spans: Arc::clone(&spans),
            });
            let _guard = tracing::subscriber::set_default(subscriber);

            actor
                .execute_tool_calls(
                    vec![tool_call(
                        "off-hit",
                        "search_code",
                        serde_json::json!({"pattern": "needle-keep", "path": note_path}),
                    )],
                    Some(MODEL.into()),
                )
                .await
                .expect("product-off execute");
            a3s_code_telemetry::session_ctx::drain_pending(std::time::Duration::from_secs(5)).await;
            let off_spans = spans.lock().clone();
            let off = span_for(&off_spans, "off-hit");
            let off_result = tool_result_text(&actor, "off-hit").await;
            assert_eq!(
                field(off, "source_status"),
                Some("succeeded"),
                "{off_result}"
            );
            assert_eq!(field(off, "tool_id"), Some("GrokBuild:grep"));
            assert_eq!(field(off, "model_id"), Some(MODEL));
            assert_eq!(field(off, "outcome"), Some("success"));
            assert!(field(off, "session_id").is_some());
            let off_id = field(off, "invocation_id").expect("off invocation");
            assert!(product.telemetry_events().iter().all(|event| {
                event.get("event_name").and_then(Value::as_str)
                    != Some("grok-shell-tool_call_completed")
                    && event
                        .get("event_metadata")
                        .and_then(|metadata| metadata.get("invocation_id"))
                        .and_then(Value::as_str)
                        != Some(off_id)
            }));

            init_product(&product, TelemetryMode::Enabled);
            actor
                .execute_tool_calls(
                    vec![
                        tool_call(
                            "hit",
                            "search_code",
                            serde_json::json!({"pattern": "needle-keep", "path": note_path}),
                        ),
                        tool_call(
                            "empty",
                            "search_code",
                            serde_json::json!({"pattern": "needle-missing", "path": note_path}),
                        ),
                        // Spawn rejects a null argument, so this is exit -1 without the tool timeout.
                        tool_call(
                            "neg",
                            "search_code",
                            serde_json::json!({"pattern": "bad\u{0000}pattern", "path": note_path}),
                        ),
                    ],
                    Some(MODEL.into()),
                )
                .await
                .expect("grep execute");
            actor
                .execute_tool_calls(
                    vec![
                        tool_call(
                            "read-tmp",
                            "read_note",
                            serde_json::json!({"target_file": note_path}),
                        ),
                        tool_call(
                            "edit-tmp",
                            "search_replace",
                            serde_json::json!({
                                "file_path": note_path,
                                "old_string": "alpha",
                                "new_string": "beta",
                            }),
                        ),
                        tool_call(
                            "write-tmp",
                            "write",
                            serde_json::json!({"file_path": out_path, "content": "CANARY_BODY"}),
                        ),
                        tool_call(
                            "read-blank",
                            "read_note",
                            serde_json::json!({"target_file": "   "}),
                        ),
                    ],
                    Some(MODEL.into()),
                )
                .await
                .expect("path execute");
            a3s_code_telemetry::session_ctx::drain_pending(std::time::Duration::from_secs(5)).await;

            let captured = spans.lock().clone();
            let rows = product.telemetry_events();
            assert_grep(&captured, &rows, "hit", "succeeded", None, "success");
            assert_grep(&captured, &rows, "empty", "empty", None, "success");
            assert_grep(
                &captured,
                &rows,
                "neg",
                "failed",
                Some("search.unclassified_exit"),
                "error",
            );
            assert_tmp(
                &captured,
                &rows,
                "read-tmp",
                "read_note",
                "GrokBuild:read_file",
            );
            assert_tmp(
                &captured,
                &rows,
                "edit-tmp",
                "search_replace",
                "GrokBuild:search_replace",
            );
            assert_no_read_profile(&captured, &rows, "edit-tmp");
            assert_tmp(&captured, &rows, "write-tmp", "write", "OpenCode:write");
            assert_no_read_profile(&captured, &rows, "write-tmp");
            let blank = span_for(&captured, "read-blank");
            let blank_id = field(blank, "invocation_id").expect("blank invocation");
            let blank_event = product_event(&rows, blank_id);
            let blank_row = row_metadata(&rows, blank_id);
            assert_eq!(blank_row.get("path_scope"), None);
            assert_eq!(meta_str(blank_row, "tool_name"), Some("read_note"));
            assert_eq!(meta_str(blank_row, "tool_id"), Some("GrokBuild:read_file"));
            assert!(!blank_event.to_string().contains("secret-project"));
            let _ = std::fs::remove_dir_all(scratch);
            let _ = std::fs::remove_dir_all(repo);
        })
        .await;
}

fn assert_no_read_profile(spans: &[CapturedSpan], rows: &[Value], call_id: &str) {
    let span = span_for(spans, call_id);
    let invocation = field(span, "invocation_id").unwrap_or_else(|| panic!("{call_id} invocation"));
    let metadata = row_metadata(rows, invocation);
    assert!(field(span, "read_file_role").is_none(), "{call_id}");
    assert!(metadata.get("read_file_role").is_none(), "{call_id}");
    assert!(metadata.get("read_limit_kind").is_none(), "{call_id}");
}

fn assert_read_agrees(spans: &[CapturedSpan], rows: &[Value], call_id: &str, canary: &str) {
    let span = span_for(spans, call_id);
    let invocation = field(span, "invocation_id").unwrap_or_else(|| panic!("{call_id} invocation"));
    let metadata = row_metadata(rows, invocation);
    for key in [
        "tool_id",
        "source_status",
        "source_reason",
        "output_limit",
        "read_file_role",
        "read_skill_match",
        "read_skill_source",
        "read_selection",
        "read_limit_kind",
        "read_lines_applicability",
        "read_lines_disposition",
        "read_bytes_applicability",
        "read_bytes_disposition",
        "read_tokens_applicability",
        "read_tokens_disposition",
    ] {
        assert_eq!(field(span, key), meta_str(metadata, key), "{call_id} {key}");
    }
    for key in [
        "read_lines_limit",
        "read_lines_observed",
        "read_bytes_limit",
        "read_bytes_observed",
        "read_tokens_limit",
        "read_tokens_observed",
        "read_source_bytes",
        "read_returned_lines",
        "read_returned_bytes",
    ] {
        match (field(span, key), metadata.get(key).and_then(Value::as_i64)) {
            (None, None) => {}
            (Some(text), Some(number)) => assert_eq!(text, number.to_string(), "{call_id} {key}"),
            (span_value, meta_value) => panic!("{call_id} {key}: {span_value:?} {meta_value:?}"),
        }
    }
    let rendered = format!("{metadata:?} {:?}", span.fields);
    assert!(!rendered.contains(canary), "{call_id} {rendered}");
}

#[serial_test::serial(tool_call_telemetry)]
#[tokio::test(flavor = "current_thread")]
async fn read_profile_agrees_on_the_product_row_and_span() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let _reset = ResetTelemetry;
            let product = a3s_code_test_support::MockInferenceServer::start()
                .await
                .expect("product");
            init_product(&product, TelemetryMode::Enabled);
            let root = std::env::temp_dir().join(format!("secret-read-dir-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let note = root.join("note.txt");
            let huge = root.join("huge.txt");
            let window = root.join("window.txt");
            let missing = root.join("SKILL.md");
            std::fs::write(&note, "hi\n").unwrap();
            let over = "Q".repeat(
                a3s_code_tools::implementations::grok_build::read_file::READ_FILE_MAX_BYTES
                    + a3s_token_estimation::BYTES_PER_TOKEN as usize,
            );
            std::fs::write(&huge, over).unwrap();
            std::fs::write(&window, "line-1-xxxxxxxx\nline-2-xxxxxxxx\nline-3-xxxxxxxx\nline-4-xxxxxxxx\nline-5-xxxxxxxx\nline-6-xxxxxxxx\n").unwrap();
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![
                ToolConfig::for_tool::<ReadFileTool>().with_name("read_note"),
                ToolConfig::for_tool::<SearchReplaceTool>(),
            ])
            .await;
            let toolset = actor.agent.borrow().tool_bridge().toolset();
            actor
                .workspace_ops
                .bind_local_session(
                    &actor.session_id_string(),
                    actor.tool_context.cwd.as_path().to_path_buf(),
                    actor.tool_context.hunk_tracker_handle.clone(),
                    toolset.clone(),
                    None,
                )
                .expect("bind_local_session");
            let spans = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::registry().with(CaptureLayer {
                spans: Arc::clone(&spans),
            });
            let _guard = tracing::subscriber::set_default(subscriber);
            let note_path = note.to_string_lossy().into_owned();
            let missing_path = missing.to_string_lossy().into_owned();
            let huge_path = huge.to_string_lossy().into_owned();
            actor
                .execute_tool_calls(
                    vec![
                        tool_call(
                            "read-ok",
                            "read_note",
                            serde_json::json!({"target_file": note_path}),
                        ),
                        tool_call(
                            "read-missing",
                            "read_note",
                            serde_json::json!({"target_file": missing_path}),
                        ),
                        tool_call(
                            "read-cap",
                            "read_note",
                            serde_json::json!({"target_file": huge_path}),
                        ),
                        tool_call(
                            "edit-other",
                            "search_replace",
                            serde_json::json!({
                                "file_path": note_path,
                                "old_string": "hi",
                                "new_string": "yo",
                            }),
                        ),
                    ],
                    Some(MODEL.into()),
                )
                .await
                .expect("read execute");
            {
                let mut resources = toolset.resources.lock().await;
                resources.insert(a3s_code_tools::types::resources::Params(
                    a3s_code_tools::implementations::grok_build::read_file::ReadFileParams {
                        max_output_bytes: Some(15),
                        ..Default::default()
                    },
                ));
                resources.insert(a3s_code_tools::types::resources::TruncationCfg(
                    a3s_code_tools::types::context::TruncationConfig {
                        max_lines_read: Some(2),
                        ..Default::default()
                    },
                ));
            }
            let window_path = window.to_string_lossy().into_owned();
            actor
                .execute_tool_calls(
                    vec![tool_call(
                        "read-both",
                        "read_note",
                        serde_json::json!({"target_file": window_path}),
                    )],
                    Some(MODEL.into()),
                )
                .await
                .expect("capped read");
            a3s_code_telemetry::session_ctx::drain_pending(std::time::Duration::from_secs(5)).await;
            let captured = spans.lock().clone();
            let rows = product.telemetry_events();
            let canary = "secret-read-dir";
            assert_read_agrees(&captured, &rows, "read-ok", canary);
            assert_read_agrees(&captured, &rows, "read-missing", canary);
            assert_read_agrees(&captured, &rows, "read-cap", canary);
            assert_read_agrees(&captured, &rows, "read-both", canary);
            assert_no_read_profile(&captured, &rows, "edit-other");

            let ok = row_metadata(
                &rows,
                field(span_for(&captured, "read-ok"), "invocation_id").expect("ok invocation"),
            );
            assert_eq!(meta_str(ok, "tool_id"), Some("GrokBuild:read_file"));
            assert_eq!(meta_str(ok, "tool_name"), Some("read_note"));
            assert_eq!(meta_str(ok, "read_file_role"), Some("ordinary"));
            assert_eq!(meta_str(ok, "source_status"), Some("succeeded"));
            assert_eq!(meta_str(ok, "read_limit_kind"), Some("none"));
            assert_eq!(meta_str(ok, "read_lines_disposition"), Some("within_limit"));
            assert_eq!(meta_str(ok, "read_tokens_disposition"), Some("within_limit"));

            let missing_row = row_metadata(
                &rows,
                field(span_for(&captured, "read-missing"), "invocation_id")
                    .expect("missing invocation"),
            );
            assert_eq!(meta_str(missing_row, "tool_id"), Some("GrokBuild:read_file"));
            assert_eq!(meta_str(missing_row, "read_file_role"), Some("skill_entry"));
            assert_eq!(meta_str(missing_row, "read_skill_match"), Some("unregistered"));
            assert_eq!(meta_str(missing_row, "source_status"), Some("failed"));
            assert_eq!(meta_str(missing_row, "source_reason"), Some("read.not_found"));
            assert_eq!(
                meta_str(missing_row, "read_tokens_disposition"),
                Some("unobserved")
            );
            assert_ne!(meta_str(missing_row, "read_limit_kind"), Some("tokens"));
            assert!(missing_row.get("read_tokens_observed").is_none());

            let capped = row_metadata(
                &rows,
                field(span_for(&captured, "read-cap"), "invocation_id").expect("cap invocation"),
            );
            assert_eq!(meta_str(capped, "read_limit_kind"), Some("tokens"));
            assert_eq!(meta_str(capped, "read_tokens_disposition"), Some("rejected"));
            assert_eq!(capped.get("read_tokens_limit").and_then(Value::as_i64), Some(25_000));
            assert!(capped.get("read_tokens_observed").is_none());
            assert_eq!(meta_str(capped, "source_status"), Some("failed"));
            assert_eq!(meta_str(capped, "source_reason"), Some("read.token_limit"));
            assert_eq!(meta_str(capped, "outcome"), Some("error"));

            let both = row_metadata(
                &rows,
                field(span_for(&captured, "read-both"), "invocation_id").expect("both invocation"),
            );
            assert_eq!(meta_str(both, "read_limit_kind"), Some("multiple"));
            assert_eq!(meta_str(both, "read_lines_disposition"), Some("truncated"));
            assert_eq!(meta_str(both, "read_bytes_disposition"), Some("truncated"));
            assert_eq!(meta_str(both, "tool_id"), Some("GrokBuild:read_file"));
            let _ = std::fs::remove_dir_all(root);
        })
        .await;
}
