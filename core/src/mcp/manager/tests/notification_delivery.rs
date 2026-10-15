#![cfg(test)]

use super::*;
use crate::mcp::McpNotification;
use std::time::Duration;

const TOKEN: &str = "mcp-progress-and-cache-913";

fn server_script() -> &'static str {
    r#"import json, pathlib, sys, threading, time
root = pathlib.Path(sys.argv[1])
count = int(sys.argv[2])
changed = threading.Event()
output = threading.Lock()
def send(value):
    with output:
        print(json.dumps(value), flush=True)
def tool_call(mid):
    for i in range(count):
        send({"jsonrpc":"2.0", "method":"notifications/progress", "params":{"progressToken":"actual-call", "progress":i + 1, "total":count}})
    changed.set()
    send({"jsonrpc":"2.0", "method":"notifications/tools/list_changed"})
    end = time.monotonic() + 15
    while not (root / "release").exists() and time.monotonic() < end:
        time.sleep(0.005)
    send({"jsonrpc":"2.0", "id":mid, "result":{"content":[{"type":"text", "text":"mcp-progress-and-cache-913"}], "isError":False}})
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion":msg["params"]["protocolVersion"], "capabilities":{"tools":{"listChanged":True}}, "serverInfo":{"name":"progress-cache", "version":"1"}}
    elif method == "tools/list":
        names = ["lookup", "added"] if changed.is_set() else ["lookup"]
        result = {"tools":[{"name":name, "description":name, "inputSchema":{"type":"object", "properties":{}}} for name in names]}
    elif method == "tools/call":
        threading.Thread(target=tool_call, args=(msg["id"],), daemon=True).start()
        continue
    else:
        result = {}
    send({"jsonrpc":"2.0", "id":msg["id"], "result":result})
"#
}

async fn exercise_delivery(observe: bool) {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.py");
    std::fs::write(&script, server_script()).unwrap();
    let manager = Arc::new(McpManager::new());
    manager
        .register_server(McpServerConfig {
            name: "progress-cache".into(),
            transport: McpTransportConfig::Stdio {
                command: if cfg!(windows) { "python" } else { "python3" }.into(),
                args: vec![
                    "-u".into(),
                    script.display().to_string(),
                    root.path().display().to_string(),
                    if observe { "2" } else { "132" }.into(),
                ],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 10,
        })
        .await;
    manager.connect("progress-cache").await.unwrap();
    let client = manager.get_client("progress-cache").await.unwrap();
    let mut notifications = if observe {
        let client = Arc::clone(&client);
        Some(
            tokio::task::spawn_blocking(move || client.notifications())
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let caller = Arc::clone(&manager);
    let call =
        tokio::spawn(async move { caller.call_tool("mcp__progress-cache__lookup", None).await });
    let progress = if let Some(rx) = notifications.as_mut() {
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut values = Vec::new();
            while let Some(notification) = rx.recv().await {
                if let McpNotification::Progress {
                    progress_token,
                    progress,
                    ..
                } = notification
                {
                    assert_eq!(progress_token, "actual-call");
                    values.push(progress);
                    if values.len() == 2 {
                        return values;
                    }
                }
            }
            values
        })
        .await
        .ok()
    } else {
        None
    };
    let refreshed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if client
                .get_cached_tools()
                .await
                .iter()
                .any(|tool| tool.name == "added")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let was_outstanding = !call.is_finished();
    std::fs::write(root.path().join("release"), b"release the original call").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), call).await;
    manager.disconnect("progress-cache").await.unwrap();
    assert!(
        was_outstanding,
        "progress/cache must arrive before the original tools/call returns"
    );
    assert!(
        refreshed,
        "automatic tools/list refresh must remain functional"
    );
    if observe {
        assert_eq!(
            progress,
            Some(vec![1.0, 2.0]),
            "host must receive both original progress notifications"
        );
    }
    let result = result.unwrap().unwrap().unwrap();
    assert_eq!(crate::mcp::result::tool_result_to_string(&result), TOKEN);
}

#[tokio::test]
async fn host_progress_and_cache_refresh_both_arrive_during_the_original_call() {
    exercise_delivery(true).await;
}

#[tokio::test]
async fn cache_refresh_and_call_completion_do_not_require_a_host_notification_observer() {
    exercise_delivery(false).await;
}
