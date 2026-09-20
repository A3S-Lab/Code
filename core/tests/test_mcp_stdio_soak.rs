//! S-MS-01: adding, calling, and removing a stdio MCP server must not leave
//! the child process or a session registration behind.

mod support;

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::{Agent, SessionOptions};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use support::python_stdio_command;

const FIXTURE_TOKEN: &str = "MCP-SOAK-TOKEN";

fn offline_config(sessions_dir: &Path) -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        sessions_dir: Some(sessions_dir.to_path_buf()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn fixture_script() -> String {
    format!(
        r#"import json, os, sys
with open(os.environ["MCP_SOAK_PID"], "w", encoding="ascii") as handle:
    handle.write(str(os.getpid()))

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

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
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "soak", "version": "0"}}}}}})
    elif method == "tools/list":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"tools": [{{"name": "ping", "description": "Return the fixture token.", "inputSchema": {{"type": "object", "properties": {{}}}} }}]}}}})
    elif method == "tools/call":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"content": [{{"type": "text", "text": "{FIXTURE_TOKEN}"}}], "isError": False}}}})
    else:
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{}}}})
"#
    )
}

fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

#[tokio::test]
#[ignore = "S-MS-01 soak: 30 stdio MCP add/call/remove cycles"]
async fn soak_mcp_stdio_add_call_remove_leaves_no_child() {
    let workspace = tempfile::tempdir().expect("workspace");
    let sessions = tempfile::tempdir().expect("sessions");
    let script_path = workspace.path().join("mcp_soak_fixture.py");
    std::fs::write(&script_path, fixture_script()).expect("script");
    let agent = Agent::from_config(offline_config(sessions.path()))
        .await
        .expect("agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-ms-01")
                    .with_model("anthropic/claude-sonnet-4-20250514"),
            ),
        )
        .await
        .expect("session");
    let baseline = session.mcp_status().await.len();

    for cycle in 0..30 {
        let pid_path = workspace.path().join(format!("mcp-soak-{cycle}.pid"));
        let mut env = HashMap::new();
        env.insert("MCP_SOAK_PID".to_string(), pid_path.display().to_string());
        let tools = session
            .add_mcp_server(McpServerConfig {
                name: "soak".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: python_stdio_command(),
                    args: vec![script_path.display().to_string()],
                },
                enabled: true,
                env,
                oauth: None,
                tool_timeout_secs: 60,
            })
            .await
            .expect("add");
        assert_eq!(tools, 1, "cycle {cycle} must publish the ping tool");
        let pid: u32 = std::fs::read_to_string(&pid_path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid");
        assert!(process_alive(pid), "fixture child must be alive after add");

        let result = session
            .tool("mcp__soak__ping", serde_json::json!({}))
            .await
            .expect("call");
        assert_eq!(result.exit_code, 0, "{}", result.output);
        assert!(
            result.output.contains(FIXTURE_TOKEN),
            "cycle {cycle} did not return the fixture token: {}",
            result.output
        );

        session.remove_mcp_server("soak").await.expect("remove");
        assert!(
            !process_alive(pid),
            "cycle {cycle} left MCP child {pid} alive"
        );
        let status = session.mcp_status().await;
        assert!(!status.contains_key("soak"));
        assert_eq!(status.len(), baseline, "mcp registration grew after remove");
    }
}
