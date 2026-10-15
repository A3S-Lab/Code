//! `tools/list_changed` cache refresh against a live stdio server.
//!
//! A server that adds tools after connect must become visible without a
//! reconnect: the client's notification refresher re-lists the tools when
//! the server pushes `notifications/tools/list_changed`. Before the
//! refresher, the cache stayed frozen at connect time (the notification was
//! parsed by the transport and then dropped).

use std::collections::HashMap;
use std::time::Duration;

use a3s_code_core::mcp::manager::McpManager;
use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};

fn fixture_script() -> String {
    r#"import json, sys

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

lines = 0
expanded = False
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue
    mid = msg["id"]
    method = msg.get("method")
    if method == "initialize":
        reply({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {"listChanged": True}}, "serverInfo": {"name": "grow", "version": "0"}}})
    elif method == "tools/list":
        if expanded:
            reply({"jsonrpc": "2.0", "id": mid, "result": {"tools": [
                {"name": "seed", "description": "Initial tool.", "inputSchema": {"type": "object", "properties": {}}},
                {"name": "grown", "description": "Tool added after connect.", "inputSchema": {"type": "object", "properties": {}}}
            ]}})
        else:
            reply({"jsonrpc": "2.0", "id": mid, "result": {"tools": [
                {"name": "seed", "description": "Initial tool.", "inputSchema": {"type": "object", "properties": {}}}
            ]}})
            # After the first listing, the server grows its tool set and
            # announces it — exactly once, unsolicited.
            expanded = True
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}) + "\n")
            sys.stdout.flush()
    else:
        reply({"jsonrpc": "2.0", "id": mid, "result": {}})
"#
    .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_list_changed_grows_the_cached_catalog_without_reconnect() {
    let workspace = tempfile::tempdir().expect("workspace");
    let script = workspace.path().join("grow_fixture.py");
    std::fs::write(&script, fixture_script()).expect("write fixture");

    let manager = McpManager::new();
    manager
        .register_server(McpServerConfig {
            name: "grow".to_owned(),
            transport: McpTransportConfig::Stdio {
                command: "python3".to_owned(),
                args: vec![script.display().to_string()],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 30,
        })
        .await;
    manager
        .connect_with_timeout("grow", Duration::from_secs(20))
        .await
        .expect("connect");

    let initial = manager.get_server_tools("grow").await;
    assert_eq!(
        initial.len(),
        1,
        "the connect-time catalog holds the seed tool: {initial:?}"
    );

    // The refresher re-lists on the pushed notification; poll until the
    // grown tool surfaces (bounded).
    let mut refreshed = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let tools = manager.get_server_tools("grow").await;
            if tools.iter().any(|tool| tool.name == "grown") {
                refreshed = tools;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the grown tool never surfaced without a reconnect");
    assert_eq!(refreshed.len(), 2, "the refresh replaces, not appends");
}
