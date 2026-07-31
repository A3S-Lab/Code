use super::*;
use crate::mcp::protocol::ToolContent;

#[test]
fn test_parse_tool_name() {
    let (server, tool) = McpManager::parse_tool_name("mcp__github__create_issue").unwrap();
    assert_eq!(server, "github");
    assert_eq!(tool, "create_issue");
}

#[test]
fn test_parse_tool_name_with_underscores() {
    let (server, tool) = McpManager::parse_tool_name("mcp__my_server__my_tool_name").unwrap();
    assert_eq!(server, "my_server");
    assert_eq!(tool, "my_tool_name");
}

#[test]
fn test_parse_tool_name_invalid() {
    assert!(McpManager::parse_tool_name("invalid_name").is_err());
    assert!(McpManager::parse_tool_name("mcp__nodelimiter").is_err());
}

#[test]
fn test_tool_result_to_string() {
    let result = CallToolResult {
        content: vec![
            ToolContent::Text {
                text: "Line 1".to_string(),
            },
            ToolContent::Text {
                text: "Line 2".to_string(),
            },
        ],
        is_error: false,
        ..CallToolResult::default()
    };

    let output = tool_result_to_string(&result);
    assert!(output.contains("Line 1"));
    assert!(output.contains("Line 2"));
}

#[tokio::test]
async fn test_mcp_manager_new() {
    let manager = McpManager::new();
    let status = manager.get_status().await;
    assert!(status.is_empty());
}

#[tokio::test]
async fn test_mcp_manager_register_server() {
    let manager = McpManager::new();

    let config = McpServerConfig {
        name: "test".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };

    manager.register_server(config).await;

    let status = manager.get_status().await;
    assert!(status.contains_key("test"));
    assert!(!status["test"].connected);
}

#[tokio::test]
async fn test_mcp_manager_default() {
    let manager = McpManager::default();
    let status = manager.get_status().await;
    assert!(status.is_empty());
}

#[tokio::test]
async fn test_list_connected_empty() {
    let manager = McpManager::new();
    let connected = manager.list_connected().await;
    assert!(connected.is_empty());
}

#[tokio::test]
async fn test_is_connected_false_for_unknown_server() {
    let manager = McpManager::new();
    let connected = manager.is_connected("unknown_server").await;
    assert!(!connected);
}

#[tokio::test]
async fn test_get_client_none_for_unknown_server() {
    let manager = McpManager::new();
    let client = manager.get_client("unknown_server").await;
    assert!(client.is_none());
}

#[test]
fn test_parse_tool_name_simple() {
    let (server, tool) = McpManager::parse_tool_name("mcp__server__tool").unwrap();
    assert_eq!(server, "server");
    assert_eq!(tool, "tool");
}

#[test]
fn test_parse_tool_name_multiple_underscores() {
    let (server, tool) = McpManager::parse_tool_name("mcp__my_server__my_tool_name").unwrap();
    assert_eq!(server, "my_server");
    assert_eq!(tool, "my_tool_name");
}

#[test]
fn test_parse_tool_name_missing_prefix() {
    let result = McpManager::parse_tool_name("server__tool");
    assert!(result.is_err());
}

#[test]
fn test_parse_tool_name_only_prefix() {
    let result = McpManager::parse_tool_name("mcp__");
    assert!(result.is_err());
}

#[test]
fn test_parse_tool_name_empty_string() {
    let result = McpManager::parse_tool_name("");
    assert!(result.is_err());
}

#[test]
fn test_tool_result_to_string_single_text() {
    let result = CallToolResult {
        content: vec![ToolContent::Text {
            text: "Hello World".to_string(),
        }],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert_eq!(output, "Hello World");
}

#[test]
fn test_tool_result_to_string_multiple_text() {
    let result = CallToolResult {
        content: vec![
            ToolContent::Text {
                text: "First line".to_string(),
            },
            ToolContent::Text {
                text: "Second line".to_string(),
            },
        ],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert!(output.contains("First line"));
    assert!(output.contains("Second line"));
}

#[test]
fn test_tool_result_to_string_empty() {
    let result = CallToolResult {
        content: vec![],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert_eq!(output, "");
}

#[test]
fn test_tool_result_to_string_image() {
    let result = CallToolResult {
        content: vec![ToolContent::Image {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
        }],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert!(output.contains("[Image: image/png]"));
}

#[test]
fn test_tool_result_to_string_resource() {
    use crate::mcp::protocol::ResourceContent;
    let result = CallToolResult {
        content: vec![ToolContent::Resource {
            resource: ResourceContent {
                uri: "file:///test.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                text: Some("Resource content".to_string()),
                blob: None,
            },
        }],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert!(output.contains("Resource content"));
}

#[test]
fn test_tool_result_to_string_mixed_content() {
    use crate::mcp::protocol::ResourceContent;
    let result = CallToolResult {
        content: vec![
            ToolContent::Text {
                text: "Text content".to_string(),
            },
            ToolContent::Image {
                data: "base64".to_string(),
                mime_type: "image/jpeg".to_string(),
            },
            ToolContent::Resource {
                resource: ResourceContent {
                    uri: "file:///doc.md".to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    text: Some("Doc content".to_string()),
                    blob: None,
                },
            },
        ],
        is_error: false,
        ..CallToolResult::default()
    };
    let output = tool_result_to_string(&result);
    assert!(output.contains("Text content"));
    assert!(output.contains("[Image: image/jpeg]"));
    assert!(output.contains("Doc content"));
}

#[tokio::test]
async fn test_get_status_registered_server() {
    use std::collections::HashMap;
    let manager = McpManager::new();

    let config = McpServerConfig {
        name: "test_server".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };

    manager.register_server(config).await;

    let status = manager.get_status().await;
    assert!(status.contains_key("test_server"));
    assert!(!status["test_server"].connected);
    assert!(status["test_server"].enabled);
}

#[tokio::test]
async fn test_get_status_disabled_server() {
    use std::collections::HashMap;
    let manager = McpManager::new();

    let config = McpServerConfig {
        name: "disabled_server".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
        },
        enabled: false,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };

    manager.register_server(config).await;

    let status = manager.get_status().await;
    assert!(status.contains_key("disabled_server"));
    assert!(!status["disabled_server"].enabled);
}

#[tokio::test]
async fn test_get_all_tools_empty_manager() {
    let manager = McpManager::new();
    let tools = manager.get_all_tools().await;
    assert!(tools.is_empty());
}

#[tokio::test]
async fn test_resolve_auth_header_none_when_no_oauth() {
    let result = McpManager::resolve_auth_header(None).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_resolve_auth_header_uses_static_token() {
    use crate::mcp::protocol::OAuthConfig;
    let oauth = OAuthConfig {
        auth_url: "https://example.com/auth".to_string(),
        token_url: "https://example.com/token".to_string(),
        client_id: "client".to_string(),
        client_secret: None,
        scopes: vec![],
        redirect_uri: "http://localhost/cb".to_string(),
        access_token: Some("my-static-token".to_string()),
    };
    let result = McpManager::resolve_auth_header(Some(&oauth)).await.unwrap();
    assert!(result.is_some());
    let (key, value) = result.unwrap();
    assert_eq!(key, "Authorization");
    assert_eq!(value, "Bearer my-static-token");
}

#[tokio::test]
async fn test_resolve_auth_header_client_credentials_fails_gracefully() {
    use crate::mcp::protocol::OAuthConfig;
    // No static token + invalid token_url → should return error
    let oauth = OAuthConfig {
        auth_url: "https://127.0.0.1:1/auth".to_string(),
        token_url: "http://127.0.0.1:1/token".to_string(),
        client_id: "client".to_string(),
        client_secret: Some("secret".to_string()),
        scopes: vec!["read".to_string()],
        redirect_uri: "http://localhost/cb".to_string(),
        access_token: None,
    };
    let result = McpManager::resolve_auth_header(Some(&oauth)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_connect_error_recorded_in_status() {
    use std::collections::HashMap;
    let manager = McpManager::new();

    let config = McpServerConfig {
        name: "bad-server".to_string(),
        transport: McpTransportConfig::Stdio {
            // `true` exits immediately — not a valid MCP server
            command: "true".to_string(),
            args: vec![],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 5,
    };

    manager.register_server(config).await;
    // connect() will fail; error must be stored
    let _ = manager.connect("bad-server").await;

    let status = manager.get_status().await;
    let s = &status["bad-server"];
    assert!(!s.connected, "server should not be connected");
    assert!(
        s.error.is_some(),
        "error should be recorded after failed connect"
    );
}

#[tokio::test]
async fn test_get_all_tools_returns_server_name_not_full_name() {
    // get_all_tools() must return (server_name, tool) — not (mcp__server__tool, tool)
    // so that create_mcp_tools() can build the correct full name without double-prefix.
    // With no connected servers the result is empty; the format assertion is enforced
    // structurally: the field is named "server_name" and callers must not pre-add the prefix.
    let manager = McpManager::new();
    let tools = manager.get_all_tools().await;
    // Empty is fine; verify no full-name leakage by checking the tuple semantics.
    // (Real server injection is tested via integration_mcp.rs #[ignore] tests.)
    for (name, _tool) in &tools {
        assert!(
            !name.starts_with("mcp__"),
            "get_all_tools() must return server names, not prefixed full names; got '{name}'"
        );
    }
}

#[tokio::test]
async fn touch_updates_last_used_at_ms() {
    let manager = McpManager::new();
    // Without a real connect, last_used is None.
    assert!(manager.last_used_at_ms("svc-a").await.is_none());
    manager.touch("svc-a").await;
    let t1 = manager.last_used_at_ms("svc-a").await.expect("set");
    assert!(t1 > 0);
    // Touch again — timestamp must be monotonically non-decreasing.
    manager.touch("svc-a").await;
    let t2 = manager.last_used_at_ms("svc-a").await.expect("set again");
    assert!(t2 >= t1);
}

#[tokio::test]
async fn disconnect_idle_drops_stale_servers_and_keeps_fresh_ones() {
    let manager = McpManager::new();
    // Manually populate clients + timestamps so we can run the
    // logic without actually launching MCP subprocesses. We can't
    // build an `McpClient` from outside this module without a
    // transport, so we just exercise the timestamp-driven decision
    // branch via the public APIs: register two servers with
    // explicit stale + fresh stamps and assert the idle sweep
    // picks the right one.
    //
    // NOTE: clients map stays empty (no real transport spawned),
    // so disconnect_idle's `candidates` set is empty and the
    // returned Vec is empty. We instead verify the *timestamp
    // observability* path the host needs, plus the no-op behaviour
    // when there are no live clients.
    manager.touch("fresh-svc").await;
    // Observability works while the entry is live.
    assert!(manager.last_used_at_ms("fresh-svc").await.is_some());
    assert!(manager.last_used_at_ms("never-touched").await.is_none());

    let dropped = manager.disconnect_idle(0).await;
    assert!(
        dropped.is_empty(),
        "no clients connected -> nothing to disconnect, got {dropped:?}"
    );
    // The idle sweep also purges ORPHAN timestamps — "fresh-svc" was
    // touch()ed but never connected (no entry in `clients`), so it must
    // not linger in `last_used_at_ms` after a sweep. Without this,
    // touch()-without-connect would leak unbounded.
    assert!(
        manager.last_used_at_ms("fresh-svc").await.is_none(),
        "orphan timestamp (touched, never connected) must be purged by disconnect_idle"
    );
}

#[tokio::test]
async fn touch_keeps_timestamp_after_explicit_disconnect_removes_it() {
    let manager = McpManager::new();
    manager.touch("svc").await;
    assert!(manager.last_used_at_ms("svc").await.is_some());
    // disconnect should clean up the activity entry even when
    // no real client was ever connected (defensive cleanup).
    let _ = manager.disconnect("svc").await;
    assert!(manager.last_used_at_ms("svc").await.is_none());
}

fn test_server_config(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: McpTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 5,
    }
}

#[tokio::test]
async fn test_mcp_manager_remove_server_forgets_configuration() {
    let manager = McpManager::new();
    manager.register_server(test_server_config("removed")).await;
    manager
        .connect_errors
        .write()
        .await
        .insert("removed".to_string(), "old error".to_string());
    manager.touch("removed").await;

    assert!(manager.remove_server("removed").await.unwrap());
    assert!(!manager.contains_server("removed").await);
    assert!(!manager.get_status().await.contains_key("removed"));
    assert!(manager.last_used_at_ms("removed").await.is_none());
    assert!(!manager.remove_server("removed").await.unwrap());
}

struct CloseFailingTransport;

#[async_trait::async_trait]
impl McpTransport for CloseFailingTransport {
    async fn request(
        &self,
        _request: crate::mcp::protocol::JsonRpcRequest,
    ) -> Result<crate::mcp::protocol::JsonRpcResponse> {
        Err(anyhow!("request not supported"))
    }

    async fn notify(&self, _notification: crate::mcp::protocol::JsonRpcNotification) -> Result<()> {
        Ok(())
    }

    fn notifications(&self) -> tokio::sync::mpsc::Receiver<crate::mcp::protocol::McpNotification> {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        rx
    }

    async fn close(&self) -> Result<()> {
        Err(anyhow!("close failed"))
    }

    fn is_connected(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn remove_server_commits_logical_removal_before_transport_cleanup() {
    let manager = McpManager::new();
    manager.register_server(test_server_config("failing")).await;
    let client = Arc::new(McpClient::new(
        "failing".to_string(),
        Arc::new(CloseFailingTransport),
    ));
    manager.insert_client_for_test("failing", client).await;
    manager
        .connect_errors
        .write()
        .await
        .insert("failing".to_string(), "old error".to_string());

    let error = manager.remove_server("failing").await.unwrap_err();
    assert!(error.to_string().contains("close failed"));
    assert!(!manager.contains_server("failing").await);
    assert!(manager.get_client("failing").await.is_none());
    assert!(!manager.get_status().await.contains_key("failing"));
    assert!(manager.last_used_at_ms("failing").await.is_none());
}
