use super::*;

#[test]
fn test_json_rpc_request_serialize() {
    let req = JsonRpcRequest::new(1, "initialize", Some(serde_json::json!({"test": true})));
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"method\":\"initialize\""));
}

#[test]
fn test_json_rpc_response_deserialize() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{"success":true}}"#;
    let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.id, Some(1));
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_json_rpc_error_deserialize() {
    let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert!(resp.error.is_some());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32600);
}

#[test]
fn test_mcp_tool_deserialize() {
    let json = r#"{
            "name": "create_issue",
            "description": "Create a GitHub issue",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["title"]
            }
        }"#;
    let tool: McpTool = serde_json::from_str(json).unwrap();
    assert_eq!(tool.name, "create_issue");
    assert!(tool.description.is_some());
}

#[test]
fn test_tool_content_text() {
    let content = ToolContent::Text {
        text: "Hello".to_string(),
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"text\":\"Hello\""));
}

#[test]
fn test_mcp_transport_config_stdio() {
    let json = r#"{
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"]
        }"#;
    let config: McpTransportConfig = serde_json::from_str(json).unwrap();
    match config {
        McpTransportConfig::Stdio { command, args } => {
            assert_eq!(command, "npx");
            assert_eq!(args.len(), 2);
        }
        _ => panic!("Expected Stdio transport"),
    }
}

#[test]
fn test_mcp_transport_config_http() {
    let json = r#"{
            "type": "http",
            "url": "https://mcp.example.com/api",
            "headers": {"Authorization": "Bearer token"}
        }"#;
    let config: McpTransportConfig = serde_json::from_str(json).unwrap();
    match config {
        McpTransportConfig::Http { url, headers } => {
            assert_eq!(url, "https://mcp.example.com/api");
            assert!(headers.contains_key("Authorization"));
        }
        _ => panic!("Expected Http transport"),
    }
}

#[test]
fn test_mcp_notification_parse() {
    let notification = JsonRpcNotification::new("notifications/tools/list_changed", None);
    let mcp_notif = McpNotification::from_json_rpc(&notification);
    match mcp_notif {
        McpNotification::ToolsListChanged => {}
        _ => panic!("Expected ToolsListChanged"),
    }
}
#[test]
fn test_json_rpc_request_new_with_params() {
    let req = JsonRpcRequest::new(1, "initialize", Some(serde_json::json!({"test": true})));
    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.id, 1);
    assert_eq!(req.method, "initialize");
    assert!(req.params.is_some());
}

#[test]
fn test_json_rpc_request_new_without_params() {
    let req = JsonRpcRequest::new(2, "ping", None);
    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.id, 2);
    assert_eq!(req.method, "ping");
    assert!(req.params.is_none());
}

#[test]
fn test_json_rpc_request_serialization() {
    let req = JsonRpcRequest::new(1, "test_method", Some(serde_json::json!({"key": "value"})));
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"method\":\"test_method\""));
    assert!(json.contains("\"params\""));
}

#[test]
fn test_json_rpc_response_with_result() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"success": true})),
        error: None,
    };
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}

#[test]
fn test_json_rpc_response_with_error() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: None,
        error: Some(JsonRpcError {
            code: -32600,
            message: "Invalid Request".to_string(),
            data: None,
        }),
    };
    assert!(resp.result.is_none());
    assert!(resp.error.is_some());
}

#[test]
fn test_json_rpc_response_both_none() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: None,
        error: None,
    };
    assert!(resp.result.is_none());
    assert!(resp.error.is_none());
}

#[test]
fn test_json_rpc_response_serialization() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(1),
        result: Some(serde_json::json!({"data": "test"})),
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"id\":1"));
    assert!(json.contains("\"result\""));
}

#[test]
fn test_json_rpc_notification_new_with_params() {
    let notif = JsonRpcNotification::new("notification", Some(serde_json::json!({"msg": "hello"})));
    assert_eq!(notif.jsonrpc, "2.0");
    assert_eq!(notif.method, "notification");
    assert!(notif.params.is_some());
}

#[test]
fn test_json_rpc_notification_new_without_params() {
    let notif = JsonRpcNotification::new("ping", None);
    assert_eq!(notif.jsonrpc, "2.0");
    assert_eq!(notif.method, "ping");
    assert!(notif.params.is_none());
}

#[test]
fn test_json_rpc_notification_serialization() {
    let notif = JsonRpcNotification::new(
        "test_notification",
        Some(serde_json::json!({"key": "value"})),
    );
    let json = serde_json::to_string(&notif).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"method\":\"test_notification\""));
    assert!(!json.contains("\"id\""));
}

#[test]
fn test_mcp_tool_serialize() {
    let tool = McpTool {
        name: "test_tool".to_string(),
        title: None,
        description: Some("A test tool".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        annotations: None,
        icons: Vec::new(),
        meta: None,
    };
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("\"name\":\"test_tool\""));
    assert!(json.contains("\"description\":\"A test tool\""));
}

#[test]
fn test_mcp_tool_without_description() {
    let json = r#"{"name":"tool","inputSchema":{"type":"object"}}"#;
    let tool: McpTool = serde_json::from_str(json).unwrap();
    assert_eq!(tool.name, "tool");
    assert!(tool.description.is_none());
}

#[test]
fn test_mcp_tool_preserves_output_schema_annotations_icons_and_meta() {
    let json = serde_json::json!({
        "name": "ocr_extract",
        "title": "Extract image text",
        "description": "Extract OCR text",
        "inputSchema": {"type": "object"},
        "outputSchema": {
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        },
        "annotations": {
            "readOnlyHint": true,
            "idempotentHint": true,
            "openWorldHint": true,
            "x-a3s-risk": "submit"
        },
        "icons": [{"src": "data:image/png;base64,AA==", "mimeType": "image/png"}],
        "_meta": {"provider": "vision"}
    });
    let tool: McpTool = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(tool.title.as_deref(), Some("Extract image text"));
    assert_eq!(tool.output_schema.as_ref().unwrap()["required"][0], "text");
    let annotations = tool.annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(true));
    assert_eq!(annotations.additional["x-a3s-risk"], "submit");
    assert_eq!(tool.icons.len(), 1);
    assert_eq!(tool.meta.as_ref().unwrap()["provider"], "vision");
    assert_eq!(serde_json::to_value(tool).unwrap(), json);
}

#[test]
fn test_mcp_resource_serialize() {
    let resource = McpResource {
        uri: "file:///test.txt".to_string(),
        name: "test.txt".to_string(),
        description: Some("Test file".to_string()),
        mime_type: Some("text/plain".to_string()),
    };
    let json = serde_json::to_string(&resource).unwrap();
    assert!(json.contains("\"uri\":\"file:///test.txt\""));
    assert!(json.contains("\"name\":\"test.txt\""));
}

#[test]
fn test_mcp_resource_deserialize() {
    let json = r#"{"uri":"file:///doc.md","name":"doc.md","mimeType":"text/markdown"}"#;
    let resource: McpResource = serde_json::from_str(json).unwrap();
    assert_eq!(resource.uri, "file:///doc.md");
    assert_eq!(resource.name, "doc.md");
    assert_eq!(resource.mime_type, Some("text/markdown".to_string()));
}

#[test]
fn test_initialize_params_serialization() {
    let params = InitializeParams {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ClientCapabilities::default(),
        client_info: ClientInfo {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
        },
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("\"protocolVersion\""));
    assert!(json.contains("\"clientInfo\""));
}

#[test]
fn test_initialize_result_serialization() {
    let result = InitializeResult {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ServerCapabilities::default(),
        server_info: ServerInfo {
            name: "test-server".to_string(),
            version: "1.0.0".to_string(),
        },
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"protocolVersion\""));
    assert!(json.contains("\"serverInfo\""));
}

#[test]
fn test_call_tool_params_serialization() {
    let params = CallToolParams {
        name: "test_tool".to_string(),
        arguments: Some(serde_json::json!({"arg1": "value1"})),
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("\"name\":\"test_tool\""));
    assert!(json.contains("\"arguments\""));
}

#[test]
fn test_call_tool_params_without_arguments() {
    let params = CallToolParams {
        name: "simple_tool".to_string(),
        arguments: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("\"name\":\"simple_tool\""));
    assert!(!json.contains("\"arguments\""));
}

#[test]
fn test_call_tool_result_serialization() {
    let result = CallToolResult {
        content: vec![ToolContent::Text {
            text: "Result".to_string(),
        }],
        is_error: false,
        ..CallToolResult::default()
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"isError\":false"));
}

#[test]
fn test_call_tool_result_error_flag() {
    let result = CallToolResult {
        content: vec![ToolContent::Text {
            text: "Error occurred".to_string(),
        }],
        is_error: true,
        ..CallToolResult::default()
    };
    assert!(result.is_error);
}

#[test]
fn test_call_tool_result_default() {
    let json = r#"{"content":[]}"#;
    let result: CallToolResult = serde_json::from_str(json).unwrap();
    assert!(!result.is_error);
}

#[test]
fn test_call_tool_result_preserves_structured_content_and_meta() {
    let value = serde_json::json!({
        "content": [{"type": "text", "text": "{\"text\":\"A3S\"}"}],
        "structuredContent": {
            "text": "A3S",
            "source": {"sha256": "abc"}
        },
        "isError": false,
        "_meta": {"requestId": "ocr-1"}
    });
    let result: CallToolResult = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        result.structured_content.as_ref().unwrap()["source"]["sha256"],
        "abc"
    );
    assert_eq!(result.meta.as_ref().unwrap()["requestId"], "ocr-1");
    assert_eq!(serde_json::to_value(result).unwrap(), value);
}

#[test]
fn test_read_resource_params_serialization() {
    let params = ReadResourceParams {
        uri: "file:///test.txt".to_string(),
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("\"uri\":\"file:///test.txt\""));
}

#[test]
fn test_read_resource_result_serialization() {
    let result = ReadResourceResult {
        contents: vec![ResourceContent {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: Some("Hello".to_string()),
            blob: None,
        }],
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"contents\""));
    assert!(json.contains("\"uri\""));
}

#[test]
fn test_list_tools_result_serialization() {
    let result = ListToolsResult {
        tools: vec![McpTool {
            name: "tool1".to_string(),
            title: None,
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            annotations: None,
            icons: Vec::new(),
            meta: None,
        }],
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"tools\""));
}

#[test]
fn test_list_resources_result_serialization() {
    let result = ListResourcesResult {
        resources: vec![McpResource {
            uri: "file:///test.txt".to_string(),
            name: "test.txt".to_string(),
            description: None,
            mime_type: None,
        }],
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"resources\""));
}

#[test]
fn test_server_capabilities_default() {
    let caps = ServerCapabilities::default();
    assert!(caps.tools.is_none());
    assert!(caps.resources.is_none());
    assert!(caps.prompts.is_none());
    assert!(caps.logging.is_none());
}

#[test]
fn test_server_capabilities_all_fields() {
    let caps = ServerCapabilities {
        tools: Some(ToolsCapability { list_changed: true }),
        resources: Some(ResourcesCapability {
            subscribe: true,
            list_changed: true,
        }),
        prompts: Some(PromptsCapability { list_changed: true }),
        logging: Some(LoggingCapability {}),
    };
    assert!(caps.tools.is_some());
    assert!(caps.resources.is_some());
    assert!(caps.prompts.is_some());
    assert!(caps.logging.is_some());
}

#[test]
fn test_client_capabilities_default() {
    let caps = ClientCapabilities::default();
    assert!(caps.roots.is_none());
    assert!(caps.sampling.is_none());
}

#[test]
fn test_client_capabilities_all_fields() {
    let caps = ClientCapabilities {
        roots: Some(RootsCapability { list_changed: true }),
        sampling: Some(SamplingCapability {}),
    };
    assert!(caps.roots.is_some());
    assert!(caps.sampling.is_some());
}

#[test]
fn test_mcp_notification_tools_list_changed() {
    let notif = JsonRpcNotification::new("notifications/tools/list_changed", None);
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::ToolsListChanged => {}
        _ => panic!("Expected ToolsListChanged"),
    }
}

#[test]
fn test_mcp_notification_resources_list_changed() {
    let notif = JsonRpcNotification::new("notifications/resources/list_changed", None);
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::ResourcesListChanged => {}
        _ => panic!("Expected ResourcesListChanged"),
    }
}

#[test]
fn test_mcp_notification_prompts_list_changed() {
    let notif = JsonRpcNotification::new("notifications/prompts/list_changed", None);
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::PromptsListChanged => {}
        _ => panic!("Expected PromptsListChanged"),
    }
}

#[test]
fn test_mcp_notification_progress() {
    let notif = JsonRpcNotification::new(
        "notifications/progress",
        Some(serde_json::json!({
            "progressToken": "token-123",
            "progress": 50.0,
            "total": 100.0
        })),
    );
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::Progress {
            progress_token,
            progress,
            total,
        } => {
            assert_eq!(progress_token, "token-123");
            assert_eq!(progress, 50.0);
            assert_eq!(total, Some(100.0));
        }
        _ => panic!("Expected Progress"),
    }
}

#[test]
fn test_mcp_notification_log() {
    let notif = JsonRpcNotification::new(
        "notifications/message",
        Some(serde_json::json!({
            "level": "error",
            "logger": "test-logger",
            "data": {"message": "test"}
        })),
    );
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::Log {
            level,
            logger,
            data,
        } => {
            assert_eq!(level, "error");
            assert_eq!(logger, Some("test-logger".to_string()));
            assert!(data.is_object());
        }
        _ => panic!("Expected Log"),
    }
}

#[test]
fn test_mcp_notification_log_edge_case_no_logger() {
    let notif = JsonRpcNotification::new(
        "notifications/message",
        Some(serde_json::json!({
            "level": "info",
            "data": "simple message"
        })),
    );
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::Log { level, logger, .. } => {
            assert_eq!(level, "info");
            assert!(logger.is_none());
        }
        _ => panic!("Expected Log"),
    }
}

#[test]
fn test_mcp_notification_log_edge_case_default_level() {
    let notif = JsonRpcNotification::new(
        "notifications/message",
        Some(serde_json::json!({
            "data": "message"
        })),
    );
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::Log { level, .. } => {
            assert_eq!(level, "info");
        }
        _ => panic!("Expected Log"),
    }
}

#[test]
fn test_mcp_notification_unknown() {
    let notif = JsonRpcNotification::new(
        "unknown/notification",
        Some(serde_json::json!({"key": "value"})),
    );
    let mcp_notif = McpNotification::from_json_rpc(&notif);
    match mcp_notif {
        McpNotification::Unknown { method, params } => {
            assert_eq!(method, "unknown/notification");
            assert!(params.is_some());
        }
        _ => panic!("Expected Unknown"),
    }
}

#[test]
fn test_tool_content_image() {
    let content = ToolContent::Image {
        data: "base64data".to_string(),
        mime_type: "image/png".to_string(),
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"image\""));
    assert!(json.contains("\"data\":\"base64data\""));
    assert!(json.contains("\"mimeType\":\"image/png\""));
}

#[test]
fn test_tool_content_resource() {
    let content = ToolContent::Resource {
        resource: ResourceContent {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: Some("content".to_string()),
            blob: None,
        },
    };
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("\"type\":\"resource\""));
    assert!(json.contains("\"uri\":\"file:///test.txt\""));
}

#[test]
fn test_mcp_server_config_default() {
    let config = McpServerConfig {
        name: "test-server".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };
    assert!(config.enabled);
    assert!(config.oauth.is_none());
}

#[test]
fn test_mcp_server_config_with_env() {
    let mut env = HashMap::new();
    env.insert("API_KEY".to_string(), "secret".to_string());
    let config = McpServerConfig {
        name: "test-server".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "node".to_string(),
            args: vec![],
        },
        enabled: true,
        env,
        oauth: None,
        tool_timeout_secs: 60,
    };
    assert!(config.env.contains_key("API_KEY"));
}

#[test]
fn test_mcp_server_config_with_oauth() {
    let config = McpServerConfig {
        name: "test-server".to_string(),
        transport: McpTransportConfig::Http {
            url: "https://api.example.com".to_string(),
            headers: HashMap::new(),
        },
        enabled: true,
        env: HashMap::new(),
        oauth: Some(OAuthConfig {
            auth_url: "https://auth.example.com".to_string(),
            token_url: "https://token.example.com".to_string(),
            client_id: "client-123".to_string(),
            client_secret: Some("secret".to_string()),
            scopes: vec!["read".to_string(), "write".to_string()],
            redirect_uri: "http://localhost:8080/callback".to_string(),
            access_token: None,
        }),
        tool_timeout_secs: 60,
    };
    assert!(config.oauth.is_some());
}

#[test]
fn test_mcp_transport_config_stdio_variant() {
    let transport = McpTransportConfig::Stdio {
        command: "python".to_string(),
        args: vec!["-m".to_string(), "server".to_string()],
    };
    match transport {
        McpTransportConfig::Stdio { command, args } => {
            assert_eq!(command, "python");
            assert_eq!(args.len(), 2);
        }
        _ => panic!("Expected Stdio"),
    }
}

#[test]
fn test_mcp_transport_config_http_variant() {
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer token".to_string());
    let transport = McpTransportConfig::Http {
        url: "https://mcp.example.com".to_string(),
        headers,
    };
    match transport {
        McpTransportConfig::Http { url, headers } => {
            assert_eq!(url, "https://mcp.example.com");
            assert!(headers.contains_key("Authorization"));
        }
        _ => panic!("Expected Http"),
    }
}

#[test]
fn test_mcp_prompt_serialize() {
    let prompt = McpPrompt {
        name: "test_prompt".to_string(),
        description: Some("A test prompt".to_string()),
        arguments: Some(vec![PromptArgument {
            name: "arg1".to_string(),
            description: Some("First argument".to_string()),
            required: true,
        }]),
    };
    let json = serde_json::to_string(&prompt).unwrap();
    assert!(json.contains("\"name\":\"test_prompt\""));
    assert!(json.contains("\"arguments\""));
}

#[test]
fn test_prompt_argument_default() {
    let json = r#"{"name":"arg"}"#;
    let arg: PromptArgument = serde_json::from_str(json).unwrap();
    assert_eq!(arg.name, "arg");
    assert!(!arg.required);
}

#[test]
fn test_oauth_config_with_static_token() {
    let json = r#"{
            "auth_url": "https://auth.example.com/authorize",
            "token_url": "https://auth.example.com/token",
            "client_id": "my-client",
            "scopes": ["read", "write"],
            "redirect_uri": "http://localhost/callback",
            "access_token": "static-token-abc123"
        }"#;
    let config: OAuthConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.client_id, "my-client");
    assert_eq!(config.access_token, Some("static-token-abc123".to_string()));
}

#[test]
fn test_oauth_config_without_static_token() {
    let json = r#"{
            "auth_url": "https://auth.example.com/authorize",
            "token_url": "https://auth.example.com/token",
            "client_id": "my-client",
            "scopes": [],
            "redirect_uri": "http://localhost/callback"
        }"#;
    let config: OAuthConfig = serde_json::from_str(json).unwrap();
    assert!(config.access_token.is_none());
}

#[test]
fn test_oauth_config_static_token_not_serialized_when_absent() {
    let config = OAuthConfig {
        auth_url: "https://example.com/auth".to_string(),
        token_url: "https://example.com/token".to_string(),
        client_id: "client".to_string(),
        client_secret: None,
        scopes: vec![],
        redirect_uri: "http://localhost/cb".to_string(),
        access_token: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains("access_token"));
}
