#![cfg(test)]
//! Opt-in real member MCP transport proof; no model or business mutation.
use a3s_code_core::mcp::{manager::McpManager, McpServerConfig, McpTransportConfig};
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
#[ignore = "requires a running MaaS proxy and authorized member credentials"]
async fn maas_live_streamable_client() {
    let username = std::env::var("A3S_CLOUD_MAAS_E2E_USERNAME").expect("test member required");
    let password = std::env::var("A3S_CLOUD_MAAS_E2E_PASSWORD").expect("test credential required");
    let manager = McpManager::new();
    manager
        .register_server(McpServerConfig {
            name: "a3s-maas".into(),
            transport: McpTransportConfig::StreamableHttp {
                url: "http://127.0.0.1:7653/mcp".into(),
                headers: HashMap::from([
                    ("X-Maas-Username".into(), username),
                    ("X-Maas-Password".into(), password),
                ]),
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        })
        .await;
    manager
        .connect("a3s-maas")
        .await
        .expect("real Core client handshake");
    let tools = manager.get_server_tools("a3s-maas").await;
    assert!(tools.iter().any(|tool| tool.name == "web_search"));
    let result = manager
        .call_tool(
            "mcp__a3s-maas__web_search",
            Some(json!({"text":"OpenAI official website"})),
        )
        .await
        .expect("real Core read-only tool call");
    assert!(!result.is_error);
    assert!(!result.content.is_empty());
    manager.disconnect("a3s-maas").await.unwrap();
}
