use super::*;
use crate::types::resources::InnerDispatch;
use crate::util::mcp_truncate::McpDumpKind;
use crate::util::query_tools::QueryTools;
use std::sync::Arc;

struct MockToolDispatch {
    expected_tool_name: String,
    return_output: ToolOutput,
}

#[async_trait::async_trait]
impl a3s_tool_runtime::ToolDispatch for MockToolDispatch {
    async fn call(
        &self,
        tool_id: a3s_tool_protocol::ToolId,
        _args: serde_json::Value,
        _ctx: a3s_tool_runtime::ToolCallContext,
    ) -> a3s_tool_runtime::ToolStream<a3s_tool_runtime::TypedToolOutput> {
        assert_eq!(tool_id.as_str(), self.expected_tool_name);
        let value = serde_json::to_value(self.return_output.clone()).unwrap();
        a3s_tool_runtime::terminal_only(Ok(a3s_tool_runtime::TypedToolOutput::from_value(
            tool_id, value,
        )))
    }
}

type SharedArgs = Arc<std::sync::Mutex<Option<serde_json::Value>>>;

struct CapturingDispatch {
    captured_args: SharedArgs,
}

#[async_trait::async_trait]
impl a3s_tool_runtime::ToolDispatch for CapturingDispatch {
    async fn call(
        &self,
        tool_id: a3s_tool_protocol::ToolId,
        args: serde_json::Value,
        _ctx: a3s_tool_runtime::ToolCallContext,
    ) -> a3s_tool_runtime::ToolStream<a3s_tool_runtime::TypedToolOutput> {
        if !matches!(
            tool_id.as_str(),
            "server__tool" | "linear__save_issue" | "linear__list_issues"
        ) {
            return a3s_tool_runtime::terminal_only(Err(a3s_tool_runtime::ToolError::not_found(
                tool_id,
                "Tool not found",
            )));
        }
        *self.captured_args.lock().unwrap() = Some(args);
        let value = serde_json::to_value(ToolOutput::Text("ok".into())).unwrap();
        a3s_tool_runtime::terminal_only(Ok(a3s_tool_runtime::TypedToolOutput::from_value(
            tool_id, value,
        )))
    }
}

struct NotFoundDispatch;

struct InvalidArgumentsDispatch;

#[async_trait::async_trait]
impl a3s_tool_runtime::ToolDispatch for NotFoundDispatch {
    async fn call(
        &self,
        tool_id: a3s_tool_protocol::ToolId,
        _args: serde_json::Value,
        _ctx: a3s_tool_runtime::ToolCallContext,
    ) -> a3s_tool_runtime::ToolStream<a3s_tool_runtime::TypedToolOutput> {
        a3s_tool_runtime::terminal_only(Err(a3s_tool_runtime::ToolError::not_found(
            tool_id,
            "Tool not found",
        )))
    }
}

#[async_trait::async_trait]
impl a3s_tool_runtime::ToolDispatch for InvalidArgumentsDispatch {
    async fn call(
        &self,
        _tool_id: a3s_tool_protocol::ToolId,
        _args: serde_json::Value,
        _ctx: a3s_tool_runtime::ToolCallContext,
    ) -> a3s_tool_runtime::ToolStream<a3s_tool_runtime::TypedToolOutput> {
        a3s_tool_runtime::terminal_only(Err(a3s_tool_runtime::ToolError::invalid_arguments(
            "local validation failed",
        )))
    }
}

fn new_ctx() -> a3s_tool_runtime::ToolCallContext {
    let call_id = a3s_tool_protocol::ToolCallId::new_v7();
    a3s_tool_runtime::ToolCallContext::new(call_id)
}

fn ctx_with_dispatch(
    dispatch: impl a3s_tool_runtime::ToolDispatch + 'static,
) -> a3s_tool_runtime::ToolCallContext {
    let mut ctx = new_ctx();
    ctx.extensions.insert(InnerDispatch(Arc::new(dispatch)));
    ctx
}

#[tokio::test]
async fn unresolved_file_forms_never_reach_local_or_gateway_dispatch() {
    for input in [
        UseToolInput::ArgumentsFile {
            tool_name: "server__tool".to_owned(),
            tool_input_file: "/tmp/args.json".into(),
        },
        UseToolInput::InvocationFile {
            file: "/tmp/call.json".into(),
        },
    ] {
        let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
        let ctx = ctx_with_dispatch_and_resources(
            CapturingDispatch {
                captured_args: captured.clone(),
            },
            gateway_resources(captured.clone(), serde_json::json!("unused")),
        );
        let error = a3s_tool_runtime::Tool::run(&UseTool, ctx, input)
            .await
            .unwrap_err();
        assert!(error.detail.contains("supported shell preparation"));
        assert_eq!(None, *captured.lock().unwrap());
    }
}

#[tokio::test]
async fn rejects_builtin_tool_names() {
    let tool = UseTool;
    let ctx = new_ctx();

    let result = a3s_tool_runtime::Tool::run(
        &tool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "read_file".into(),
            tool_input: serde_json::json!({}),
        }),
    )
    .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind, a3s_tool_runtime::ToolErrorKind::InvalidArguments);
    assert!(err.detail.contains("not a valid MCP tool name"));
    assert!(err.detail.contains("read_file"));
}

#[tokio::test]
async fn errors_when_inner_dispatch_not_set() {
    let tool = UseTool;
    let ctx = new_ctx();

    let result = a3s_tool_runtime::Tool::run(
        &tool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "linear__save_issue".into(),
            tool_input: serde_json::json!({}),
        }),
    )
    .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind, a3s_tool_runtime::ToolErrorKind::InvalidArguments);
    assert!(err.detail.contains("inner_dispatch not set"));
}

#[tokio::test]
async fn dispatches_via_ctx_inner_dispatch() {
    let tool = UseTool;
    let ctx = ctx_with_dispatch(MockToolDispatch {
        expected_tool_name: "linear__save_issue".into(),
        return_output: ToolOutput::Text("issue created".into()),
    });

    let result = a3s_tool_runtime::Tool::run(
        &tool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "linear__save_issue".into(),
            tool_input: serde_json::json!({"title": "test issue"}),
        }),
    )
    .await;

    let ToolOutput::Text(msg) = result.unwrap() else {
        panic!("expected text output");
    };
    assert_eq!("issue created", msg.text);
}

#[tokio::test]
async fn propagates_inner_dispatch_error() {
    let tool = UseTool;
    let ctx = ctx_with_dispatch(NotFoundDispatch);

    let result = a3s_tool_runtime::Tool::run(
        &tool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "bad__tool".into(),
            tool_input: serde_json::json!({}),
        }),
    )
    .await;

    let err = result.unwrap_err();
    assert_eq!(a3s_tool_runtime::ToolErrorKind::NotFound, err.kind);
}

#[derive(Clone)]
struct MockGatewayCaller {
    captured: SharedArgs,
    result: serde_json::Value,
    expected_call_id: Option<&'static str>,
}

#[async_trait::async_trait]
impl crate::types::resources::ManagedGatewayToolCaller for MockGatewayCaller {
    async fn call_tool(
        &self,
        call_id: &str,
        arguments: serde_json::Value,
        _caller: &str,
    ) -> Result<crate::types::resources::ManagedGatewayToolCallResponse, a3s_tool_runtime::ToolError>
    {
        if let Some(expected) = self.expected_call_id {
            assert_eq!(call_id, expected);
        }
        *self.captured.lock().unwrap() = Some(arguments);
        Ok(crate::types::resources::ManagedGatewayToolCallResponse {
            result: self.result.clone(),
            connectors_needing_reauth: vec![],
        })
    }
}

fn gateway_resources(
    captured: SharedArgs,
    result: serde_json::Value,
) -> crate::types::resources::SharedResources {
    gateway_resources_with_expected_call_id(captured, result, Some("grafana.searchDashboards"))
}

fn gateway_resources_with_expected_call_id(
    captured: SharedArgs,
    result: serde_json::Value,
    expected_call_id: Option<&'static str>,
) -> crate::types::resources::SharedResources {
    use crate::types::resources::{
        ManagedGatewayToolCatalog, ManagedGatewayToolClient, ManagedGatewayToolSource, Resources,
    };
    let mut resources = Resources::new();
    resources.insert(ManagedGatewayToolCatalog(std::collections::HashMap::from(
        [
            (
                "grafana__search_dashboards".to_string(),
                ManagedGatewayToolSource {
                    connector_id: "grafana".to_string(),
                    connector_name: "Grafana".to_string(),
                    tool_id: "search_dashboards".to_string(),
                    tool_name: "Search Dashboards".to_string(),
                    call_id: "grafana.searchDashboards".to_string(),
                },
            ),
            (
                "server__tool".to_string(),
                ManagedGatewayToolSource {
                    connector_id: "server".to_string(),
                    connector_name: "Gateway Collision".to_string(),
                    tool_id: "tool".to_string(),
                    tool_name: "Tool".to_string(),
                    call_id: "gateway.collision".to_string(),
                },
            ),
            (
                "connector__bad/id".to_string(),
                ManagedGatewayToolSource {
                    connector_id: "connector".to_string(),
                    connector_name: "Gateway Invalid Local".to_string(),
                    tool_id: "bad/id".to_string(),
                    tool_name: "Bad ID".to_string(),
                    call_id: "gateway.invalidLocal".to_string(),
                },
            ),
        ],
    )));
    resources.insert(ManagedGatewayToolClient(Arc::new(MockGatewayCaller {
        captured,
        result,
        expected_call_id,
    })));
    resources.into_shared()
}

#[tokio::test]
async fn gateway_tool_dispatches_to_gateway_call_id() {
    let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
    let ctx = ctx_with_dispatch_and_resources(
        NotFoundDispatch,
        gateway_resources(
            Arc::clone(&captured),
            serde_json::json!({"content": [{"type": "text", "text": "dashboards"}]}),
        ),
    );

    let result = a3s_tool_runtime::Tool::run(
        &UseTool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "grafana__search_dashboards".into(),
            tool_input: serde_json::json!({"query": "prod"}),
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        captured.lock().unwrap().clone().unwrap().get("query"),
        Some(&serde_json::json!("prod"))
    );
    if let ToolOutput::MCP(mcp) = result {
        match mcp.output() {
            crate::types::output::MCPOutputDetails::OkayOutput(text) => {
                assert_eq!(text, "dashboards")
            }
            _ => panic!("expected okay output"),
        }
    } else {
        panic!("expected gateway result to map to MCP output");
    }
}

#[tokio::test]
async fn gateway_error_spellings_map_to_mcp_errors() {
    for key in ["isError", "is_error"] {
        let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
        let ctx = ctx_with_dispatch_and_resources(
            NotFoundDispatch,
            gateway_resources(
                captured,
                serde_json::json!({
                    key: true, "content": [{"type":"text","text":"remote failed"}],
                }),
            ),
        );
        let output = a3s_tool_runtime::Tool::run(
            &UseTool,
            ctx,
            UseToolInput::Inline(InlineMcpInvocation {
                tool_name: "grafana__search_dashboards".to_owned(),
                tool_input: serde_json::json!({}),
            }),
        )
        .await
        .unwrap();
        assert!(output.is_error());
        assert!(
            output
                .to_prompt_format()
                .contains("Failed to call grafana__search_dashboards: remote failed")
        );
    }
}

#[tokio::test]
async fn gateway_call_result_converts_to_model_visible_output() {
    let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
    let ctx = ctx_with_dispatch_and_resources(
        NotFoundDispatch,
        gateway_resources(Arc::clone(&captured), serde_json::json!({"ok": true})),
    );

    let result = a3s_tool_runtime::Tool::run(
        &UseTool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "grafana__search_dashboards".into(),
            tool_input: serde_json::json!({}),
        }),
    )
    .await
    .unwrap();

    assert!(result.to_prompt_format().contains("\"ok\": true"));
}

#[test]
fn gateway_structured_content_is_appended_when_content_is_only_a_summary() {
    let folders = serde_json::json!({"folders": [{"id": "p1", "name": "Alpha"}]});
    for key in ["structuredContent", "structured_content"] {
        let text = gateway_result_to_text(serde_json::json!({
            "content": [{"type": "text", "text": "7 product folders, 2 custom folders"}],
            key: folders,
        }));
        assert_eq!(
            text,
            format!("7 product folders, 2 custom folders\n{folders}")
        );
    }
}

#[test]
fn gateway_structured_content_with_empty_or_missing_content_is_the_whole_text() {
    let folders = serde_json::json!({"folders": [{"id": "p1", "name": "Alpha"}]});
    for result in [
        serde_json::json!({"content": [], "structuredContent": folders}),
        serde_json::json!({"structuredContent": folders, "isError": false}),
    ] {
        assert_eq!(gateway_result_to_text(result), folders.to_string());
    }
}

#[test]
fn gateway_inlined_structured_content_is_not_duplicated() {
    let folders = serde_json::json!({"folders": [{"id": "p1", "name": "Alpha"}]});
    let text = gateway_result_to_text(serde_json::json!({
        "content": [{"type": "text", "text": folders.to_string()}],
        "structuredContent": folders,
    }));
    assert_eq!(text, folders.to_string());
}

#[test]
fn gateway_unknown_content_blocks_keep_the_envelope_with_structured_content() {
    let text = gateway_result_to_text(serde_json::json!({
        "content": [{"type": "resource_link", "uri": "file:///a"}],
        "structuredContent": {"count": 1},
    }));
    assert!(text.contains("file:///a"), "{text}");
    assert!(text.contains("\"count\": 1"), "{text}");
}

#[tokio::test]
async fn inline_argument_normalization_matches_local_and_gateway_dispatch() {
    for gateway in [false, true] {
        for (input, expected) in [
            (serde_json::Value::Null, serde_json::json!({})),
            (
                serde_json::json!("{\"assignee\":\"me\",\"limit\":10}"),
                serde_json::json!({"assignee":"me","limit":10}),
            ),
            (
                serde_json::json!({"title":"test","team":"ENG"}),
                serde_json::json!({"title":"test","team":"ENG"}),
            ),
            (serde_json::json!("not json"), serde_json::json!("not json")),
        ] {
            let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
            let (ctx, tool_name) = if gateway {
                (
                    ctx_with_dispatch_and_resources(
                        NotFoundDispatch,
                        gateway_resources(captured.clone(), serde_json::json!("ok")),
                    ),
                    "grafana__search_dashboards",
                )
            } else {
                (
                    ctx_with_dispatch(CapturingDispatch {
                        captured_args: captured.clone(),
                    }),
                    "server__tool",
                )
            };
            a3s_tool_runtime::Tool::run(
                &UseTool,
                ctx,
                UseToolInput::Inline(InlineMcpInvocation {
                    tool_name: tool_name.to_owned(),
                    tool_input: input,
                }),
            )
            .await
            .unwrap();
            assert_eq!(Some(expected), *captured.lock().unwrap());
        }
    }
}

#[tokio::test]
async fn gateway_tool_with_invalid_local_tool_id_falls_back_to_gateway() {
    let gateway_captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
    let ctx = ctx_with_dispatch_and_resources(
        NotFoundDispatch,
        gateway_resources_with_expected_call_id(
            Arc::clone(&gateway_captured),
            serde_json::json!("gateway ran"),
            Some("gateway.invalidLocal"),
        ),
    );

    let result = a3s_tool_runtime::Tool::run(
        &UseTool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "connector__bad/id".into(),
            tool_input: serde_json::json!({"q": "x"}),
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        gateway_captured.lock().unwrap().clone().unwrap().get("q"),
        Some(&serde_json::json!("x"))
    );
    assert!(matches!(result, ToolOutput::MCP(_)));
}

#[tokio::test]
async fn gateway_catalog_collision_propagates_local_non_not_found_error() {
    let gateway_captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
    let ctx = ctx_with_dispatch_and_resources(
        InvalidArgumentsDispatch,
        gateway_resources(
            Arc::clone(&gateway_captured),
            serde_json::json!("gateway should not run"),
        ),
    );

    let result = a3s_tool_runtime::Tool::run(
        &UseTool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "server__tool".into(),
            tool_input: serde_json::json!({"local": true}),
        }),
    )
    .await;

    let err = result.unwrap_err();
    assert_eq!(err.kind, a3s_tool_runtime::ToolErrorKind::InvalidArguments);
    assert!(err.detail.contains("local validation failed"));
    assert!(gateway_captured.lock().unwrap().is_none());
}

#[tokio::test]
async fn gateway_catalog_collision_prefers_local_dispatch_for_server_tool() {
    let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
    let ctx = ctx_with_dispatch_and_resources(
        CapturingDispatch {
            captured_args: Arc::clone(&captured),
        },
        gateway_resources(
            Arc::new(std::sync::Mutex::new(None)),
            serde_json::json!("gateway should not run"),
        ),
    );

    let result = a3s_tool_runtime::Tool::run(
        &UseTool,
        ctx,
        UseToolInput::Inline(InlineMcpInvocation {
            tool_name: "server__tool".into(),
            tool_input: serde_json::json!({"local": true}),
        }),
    )
    .await;

    result.unwrap();
    assert_eq!(
        Some(serde_json::json!({"local": true})),
        *captured.lock().unwrap()
    );
}

fn ctx_with_dispatch_and_resources(
    dispatch: impl a3s_tool_runtime::ToolDispatch + 'static,
    resources: crate::types::resources::SharedResources,
) -> a3s_tool_runtime::ToolCallContext {
    let mut ctx = new_ctx();
    ctx.extensions.insert(InnerDispatch(Arc::new(dispatch)));
    ctx.extensions.insert(resources);
    ctx
}

#[tokio::test]
async fn truncation_limits_apply_to_success_and_error_outputs() {
    use crate::types::context::TruncationConfig;
    use crate::types::output::{MCPOutput, MCPOutputDetails};
    use crate::types::resources::{Resources, TruncationCfg};
    use crate::util::truncate::format_bytes;

    for limit in [20_000, 5_000] {
        for is_error in [false, true] {
            let content = "x".repeat(limit + 1000);
            let output = if is_error {
                MCPOutput::errored("server__tool".into(), "server".into(), content)
            } else {
                MCPOutput::okay_output("server__tool".into(), "server".into(), content)
            };
            let mut cfg = TruncationConfig::default();
            cfg.per_tool_max_output_bytes
                .insert("use_tool".to_owned(), limit);
            let mut resources = Resources::new();
            resources.insert(TruncationCfg(cfg));
            let ctx = ctx_with_dispatch_and_resources(
                MockToolDispatch {
                    expected_tool_name: "server__tool".to_owned(),
                    return_output: ToolOutput::MCP(output),
                },
                resources.into_shared(),
            );
            let output = a3s_tool_runtime::Tool::run(
                &UseTool,
                ctx,
                UseToolInput::Inline(InlineMcpInvocation {
                    tool_name: "server__tool".to_owned(),
                    tool_input: serde_json::json!({}),
                }),
            )
            .await
            .unwrap();
            assert_eq!(is_error, output.is_error());
            let ToolOutput::MCP(mcp) = output else {
                panic!("expected MCP output");
            };
            let text = match mcp.output() {
                MCPOutputDetails::OkayOutput(text) | MCPOutputDetails::Error(text) => text,
            };
            assert!(text.contains("[MCP output truncated:"));
            assert!(text.contains(&format!("showing first {}", format_bytes(limit as u64))));
        }
    }
}

#[test]
fn schema_allows_arbitrary_properties_for_tool_input() {
    let schema = schemars::schema_for!(UseToolInput);
    let schema_json = serde_json::to_value(&schema).unwrap();
    assert_eq!(
        Some(&serde_json::json!(["tool_name", "tool_input"])),
        schema_json.get("required")
    );
    assert!(!schema_json.to_string().contains("file"));
    let manifest = crate::registry::types::ToolRegistryBuilder::new().get_tools_config_raw();
    assert!(
        !manifest
            .pointer("/GrokBuild:use_tool/input_schema")
            .unwrap()
            .to_string()
            .contains("file")
    );
    let description = a3s_tool_runtime::Tool::description(
        &UseTool,
        &a3s_tool_runtime::ListToolsContext::default(),
    );
    assert!(!description.description.to_lowercase().contains("file"));
    let Some(tool_input_schema) = schema_json.pointer("/properties/tool_input") else {
        panic!("schema missing tool_input: {schema_json}");
    };
    assert_eq!(
        tool_input_schema.get("type"),
        Some(&serde_json::json!("object")),
        "tool_input schema should have type: object, got: {tool_input_schema}"
    );
    assert_eq!(
        tool_input_schema.get("additionalProperties"),
        Some(&serde_json::json!(true)),
        "tool_input schema must allow arbitrary keys for MCP inputs, got: {tool_input_schema}"
    );
}

#[test]
fn dump_classification_preserves_shape_and_query_steer() {
    let row = "{'id': 0, 'name': 'user0', 'email': 'u0@example.com', 'age': 20}";
    for (payload, expected) in [
        (
            format!(r#"{{"data":"{}"}}"#, "x".repeat(3_000)),
            McpDumpKind::LongLineJson,
        ),
        (
            format!("[{}]", vec!["\"x\""; 1_000].join(",")),
            McpDumpKind::LongLineJson,
        ),
        ("{\n  \"name\": \"node\"\n}".to_owned(), McpDumpKind::Json),
        ("   \n {\"a\":1} \n".to_owned(), McpDumpKind::Json),
        (
            "just some log output\nline two\nline three".to_owned(),
            McpDumpKind::Other,
        ),
        ("{not valid json".to_owned(), McpDumpKind::Other),
        ("[1, 2, 3".to_owned(), McpDumpKind::Other),
        (
            "id,name,age\n0,user0,20\n1,user1,21".to_owned(),
            McpDumpKind::Other,
        ),
        (
            format!("[{}]", vec![row; 60].join(", ")),
            McpDumpKind::LongLineText,
        ),
        ("QUJD".repeat(800), McpDumpKind::LongLineText),
        ("12345".to_owned(), McpDumpKind::Other),
        ("true".to_owned(), McpDumpKind::Other),
        ("null".to_owned(), McpDumpKind::Other),
        ("\"a string\"".to_owned(), McpDumpKind::Other),
        (String::new(), McpDumpKind::Other),
    ] {
        let kind = McpDumpKind::classify(&payload);
        assert_eq!(expected, kind, "{payload}");
        let is_json = matches!(kind, McpDumpKind::Json | McpDumpKind::LongLineJson);
        assert_eq!(if is_json { "json" } else { "txt" }, kind.extension());
        let steer = kind.steer(
            "shell_tool",
            QueryTools {
                jq: Some("jq"),
                python: Some("python3"),
                sed: Some("sed"),
                cut: Some("cut"),
            },
        );
        if kind == McpDumpKind::Other {
            assert!(steer.is_empty());
        } else {
            assert!(steer.contains("shell_tool"));
            assert_eq!(kind != McpDumpKind::Json, steer.contains("grep"));
            assert!(steer.contains(if is_json { "jq" } else { "python" }));
            assert!(!steer.contains("if available"));
            if kind == McpDumpKind::LongLineText {
                assert!(!steer.contains("valid JSON"));
            }
        }
    }
}

#[test]
fn steer_names_only_installed_tools() {
    let tools = QueryTools {
        jq: None,
        python: Some("python3"),
        sed: None,
        cut: None,
    };
    let steer = McpDumpKind::LongLineJson.steer("bash", tools);
    assert!(
        steer.contains("python3"),
        "names the present python: {steer}"
    );
    assert!(!steer.contains("jq"), "must not name absent jq: {steer}");
    assert!(
        !steer.contains("if available"),
        "no hedge once presence is known: {steer}"
    );
}

#[test]
fn steer_omits_examples_when_no_query_tools_present() {
    let none = QueryTools::default();
    let steer = McpDumpKind::LongLineJson.steer("bash", none);
    assert!(
        steer.contains("`bash`"),
        "still names the shell tool: {steer}"
    );
    assert!(
        !steer.contains("(e.g."),
        "no examples when none present: {steer}"
    );
    assert!(
        !steer.contains("jq") && !steer.contains("python"),
        "{steer}"
    );
    assert!(
        steer.contains("grep"),
        "keeps the long-line warning: {steer}"
    );
}

#[tokio::test]
async fn truncated_json_steer_requires_a_saved_dump() {
    use crate::types::context::TruncationConfig;
    use crate::types::output::{MCPOutput, MCPOutputDetails};
    use crate::types::resources::{Resources, SessionFolder, TruncationCfg};

    for save_dump in [false, true] {
        let tmp = tempfile::TempDir::new().unwrap();
        let limit = 20_000;
        let mut resources = Resources::new();
        if save_dump {
            resources.insert(SessionFolder(tmp.path().to_path_buf()));
        }
        let mut cfg = TruncationConfig::default();
        cfg.per_tool_max_output_bytes
            .insert("use_tool".to_owned(), limit);
        resources.insert(TruncationCfg(cfg));
        let ctx = ctx_with_dispatch_and_resources(
            MockToolDispatch {
                expected_tool_name: "server__tool".to_owned(),
                return_output: ToolOutput::MCP(MCPOutput::okay_output(
                    "server__tool".to_owned(),
                    "server".to_owned(),
                    format!("[{}]", vec!["1"; limit].join(",")),
                )),
            },
            resources.into_shared(),
        );
        let output = a3s_tool_runtime::Tool::run(
            &UseTool,
            ctx,
            UseToolInput::Inline(InlineMcpInvocation {
                tool_name: "server__tool".to_owned(),
                tool_input: serde_json::json!({}),
            }),
        )
        .await
        .unwrap();
        let ToolOutput::MCP(mcp) = output else {
            panic!("expected MCP output");
        };
        let MCPOutputDetails::OkayOutput(text) = mcp.output() else {
            panic!("expected successful output");
        };
        assert!(text.contains("[MCP output truncated:"));
        assert_eq!(save_dump, text.contains("to query the saved file"));
        assert!(!text.contains("if available"));
        if save_dump {
            let files: Vec<_> = std::fs::read_dir(tmp.path().join("mcp"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            assert_eq!(1, files.len());
            assert_eq!(
                Some("json"),
                files
                    .first()
                    .unwrap()
                    .extension()
                    .and_then(|extension| extension.to_str())
            );
            assert!(text.contains(".json. ") && text.contains("`bash`"));
        } else {
            assert!(!text.contains("ineffective on it"));
        }
    }
}

#[tokio::test]
async fn native_and_unknown_names_never_dispatch() {
    use crate::types::resources::{EnabledNativeToolNames, Params, Resources};
    for (name, correction, expected) in [
        ("scheduler_create", true, "native tool"),
        ("jira", true, "not a valid MCP tool name"),
        ("scheduler_create", false, "not a valid MCP tool name"),
    ] {
        let captured: SharedArgs = Arc::new(std::sync::Mutex::new(None));
        let mut resources = Resources::new();
        resources.insert(EnabledNativeToolNames(std::collections::HashSet::from([
            "scheduler_create".to_owned(),
        ])));
        resources.insert(Params(UseToolParams {
            native_tool_correction: correction,
            ..UseToolParams::default()
        }));
        let ctx = ctx_with_dispatch_and_resources(
            CapturingDispatch {
                captured_args: captured.clone(),
            },
            resources.into_shared(),
        );
        let error = a3s_tool_runtime::Tool::run(
            &UseTool,
            ctx,
            UseToolInput::Inline(InlineMcpInvocation {
                tool_name: name.to_owned(),
                tool_input: serde_json::json!({}),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            a3s_tool_runtime::ToolErrorKind::InvalidArguments,
            error.kind
        );
        assert!(error.detail.contains(expected), "{}", error.detail);
        assert_eq!(
            correction && name == "scheduler_create",
            error.detail.contains("native tool")
        );
        if correction && name == "scheduler_create" {
            assert!(error.detail.contains(name) && error.detail.contains("directly"));
        }
        assert!(captured.lock().unwrap().is_none());
    }
}
