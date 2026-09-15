//! Live E2E coverage for first-principles issue fixes #139 / #137 / #138.
//!
//! Pins `boyue/bailian/deepseek-v4.1-flash` from monorepo `.a3s/config.acl`.
//! Passes require kernel effects (tool names, MCP progress delivery, event-page
//! projection). Assistant wording is never a pass criterion.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_issue_fix_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::mcp::manager::McpManager;
use a3s_code_core::mcp::protocol::{McpNotification, McpServerConfig, McpTransportConfig};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::tools::{ToolResultTransformPolicyV1, MAX_OUTPUT_SIZE};
use a3s_code_core::{
    Agent, AgentEvent, AgentProtocolEventPageV1, AgentProtocolRunIdentityV1, RunStatus,
    SessionOptions, AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES, AGENT_PROTOCOL_V1,
};
use support::layer_c_model::{load_pinned_layer_c_config, REQUIRED_DEFAULT_MODEL};

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);
const WRITE_TOKEN: &str = "issue139-write-token-c4e1";
const MCP_TOKEN: &str = "issue137-mcp-token-9b2f";
const LARGE_MARKER: &str = "issue138-large-marker-7d01";

async fn configured_agent() -> Agent {
    let config = load_pinned_layer_c_config();
    assert_eq!(
        config.default_model.as_deref(),
        Some(REQUIRED_DEFAULT_MODEL),
        "live issue-fix suite must pin {REQUIRED_DEFAULT_MODEL}"
    );
    Agent::from_config(config)
        .await
        .expect("build agent from .a3s/config.acl")
}

fn policy(allows: &[&str]) -> PermissionPolicy {
    let mut policy = PermissionPolicy {
        default_decision: PermissionDecision::Deny,
        ..PermissionPolicy::default()
    }
    .allow("read(**)");
    for rule in allows {
        policy = policy.allow(*rule);
    }
    policy
}

fn options(session_id: &str, allows: &[&str]) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy(allows))
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(10)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

struct Observed {
    tool_starts: Vec<(String, String)>,
    tool_ends: Vec<(String, i32, String, Option<serde_json::Value>)>,
    errors: Vec<String>,
    end_text: String,
}

async fn observe(session: &a3s_code_core::AgentSession, prompt: &str) -> Observed {
    let (mut events, join) = session.stream(prompt, None).await.expect("stream");
    let observed = tokio::time::timeout(MODEL_TIMEOUT, async {
        let mut observed = Observed {
            tool_starts: Vec::new(),
            tool_ends: Vec::new(),
            errors: Vec::new(),
            end_text: String::new(),
        };
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolStart { id, name } => {
                    observed.tool_starts.push((id, name));
                }
                AgentEvent::ToolEnd {
                    name,
                    exit_code,
                    output,
                    metadata,
                    ..
                } => {
                    observed.tool_ends.push((name, exit_code, output, metadata));
                }
                AgentEvent::Error { message, .. } => {
                    observed.errors.push(message);
                }
                AgentEvent::End { text, .. } => {
                    observed.end_text = text;
                }
                _ => {}
            }
        }
        observed
    })
    .await
    .expect("model timeout");
    let _ = join.await;
    observed
}

fn release_digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

fn protocol_identity(session_id: &str, run_id: &str) -> AgentProtocolRunIdentityV1 {
    AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_digest(),
        session_id: session_id.into(),
        run_id: run_id.into(),
    }
}

/// #139: multi-step tool streaming must keep non-empty tool names across turns.
/// Gateways that re-send `"name":""` on argument deltas previously wiped the
/// accumulated name and poisoned the next request with a 400.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/bailian/deepseek-v4.1-flash from .a3s/config.acl"]
async fn live_streaming_tool_names_survive_multi_step_rounds() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options(
                "issue-139-stream",
                &["write(**)", "bash(**)", "read(**)"],
            )),
        )
        .await
        .expect("session");

    let first = observe(
        &session,
        &format!(
            "Create file token.txt containing exactly `{WRITE_TOKEN}` using the write tool. Then stop."
        ),
    )
    .await;
    assert!(
        first
            .errors
            .iter()
            .all(|message| !looks_like_empty_tool_name_poison(message)),
        "first turn must not show empty function.name poison: {:?}",
        first.errors
    );
    assert!(
        !first.tool_starts.is_empty(),
        "first turn must start at least one tool: starts={:?} ends={:?}",
        first.tool_starts,
        first
            .tool_ends
            .iter()
            .map(|(n, c, _, _)| (n.as_str(), *c))
            .collect::<Vec<_>>()
    );
    assert!(
        first
            .tool_starts
            .iter()
            .all(|(id, name)| !id.is_empty() && !name.is_empty()),
        "ToolStart id/name must stay non-empty after streaming accumulation: {:?}",
        first.tool_starts
    );
    let wrote = first.tool_ends.iter().any(|(name, code, output, _)| {
        (name == "write" && *code == 0)
            || (name == "bash" && *code == 0 && output.contains(WRITE_TOKEN))
    });
    let on_disk = std::fs::read_to_string(workspace.path().join("token.txt"))
        .unwrap_or_default()
        .contains(WRITE_TOKEN);
    assert!(
        wrote || on_disk,
        "first turn must create token.txt with {WRITE_TOKEN}: ends={:?} disk={}",
        first
            .tool_ends
            .iter()
            .map(|(n, c, o, _)| (n.as_str(), *c, o.as_str()))
            .collect::<Vec<_>>(),
        on_disk
    );

    let second = observe(
        &session,
        "Read token.txt with the read tool and return only the file contents. Do not invent the token.",
    )
    .await;
    assert!(
        second
            .errors
            .iter()
            .all(|message| !looks_like_empty_tool_name_poison(message)),
        "second turn must not fail from empty function.name wipe: {:?}",
        second.errors
    );
    assert!(
        second
            .tool_starts
            .iter()
            .all(|(id, name)| !id.is_empty() && !name.is_empty()),
        "second-turn ToolStart names must stay non-empty: {:?}",
        second.tool_starts
    );
    let read_ok = second.tool_ends.iter().any(|(name, code, output, _)| {
        name == "read" && *code == 0 && output.contains(WRITE_TOKEN)
    });
    assert!(
        read_ok,
        "second turn must read the written token via tools: {:?}",
        second
            .tool_ends
            .iter()
            .map(|(n, c, o, _)| (n.as_str(), *c, o.as_str()))
            .collect::<Vec<_>>()
    );
}

fn looks_like_empty_tool_name_poison(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("function.name")
        || (lower.contains("tool_calls") && lower.contains("invalid"))
        || (lower.contains("empty") && lower.contains("tool") && lower.contains("name"))
}

/// #137: stdio MCP progress notifications must reach the client while a live
/// model-driven tools/call is outstanding.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/bailian/deepseek-v4.1-flash from .a3s/config.acl"]
async fn live_mcp_stdio_progress_notifications_arrive_during_tool_call() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let script = workspace.path().join("progress_mcp.py");
    std::fs::write(&script, progress_mcp_script()).expect("mcp script");

    // Inherit a pre-connected manager so live tools/call and notification
    // subscription share one stdio client (with_mcp is inherited, not the
    // session-private add_mcp_server target).
    let manager = Arc::new(McpManager::new());
    manager
        .register_server(McpServerConfig {
            name: "progress".into(),
            transport: McpTransportConfig::Stdio {
                command: "python3".into(),
                args: vec![script.to_string_lossy().into_owned()],
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 30,
        })
        .await;
    manager
        .connect("progress")
        .await
        .expect("connect progress MCP before session");
    let client = manager
        .get_client("progress")
        .await
        .expect("progress MCP client after connect");
    let mut notification_rx = tokio::task::spawn_blocking(move || client.notifications())
        .await
        .expect("take notification receiver");

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("issue-137-mcp", &["mcp__progress__*"])
                    .with_mcp(Arc::clone(&manager))
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");

    let progress_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_task = {
        let progress_counter = Arc::clone(&progress_counter);
        tokio::spawn(async move {
            while let Some(notification) = notification_rx.recv().await {
                if matches!(notification, McpNotification::Progress { .. }) {
                    progress_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        })
    };

    let observed = observe(
        &session,
        "Call mcp__progress__lookup exactly once and return only the tool token. The token is not in this message.",
    )
    .await;
    // Give the reader a beat to drain late frames after tools/call returns.
    tokio::time::sleep(Duration::from_millis(200)).await;
    progress_task.abort();

    let called = observed.tool_ends.iter().any(|(name, code, output, _)| {
        name == "mcp__progress__lookup" && *code == 0 && output.contains(MCP_TOKEN)
    });
    assert!(
        called,
        "live model must call the progress MCP tool: {:?}",
        observed
            .tool_ends
            .iter()
            .map(|(n, c, o, _)| (n.as_str(), *c, o.as_str()))
            .collect::<Vec<_>>()
    );
    let progress_count = progress_counter.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        progress_count >= 2,
        "stdio reader must deliver >=2 notifications/progress during the live tools/call (got {progress_count})"
    );
}

/// #138: a live large tool_end must still project through agent_protocol event
/// pages instead of permanently 400-ing the cursor.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/bailian/deepseek-v4.1-flash from .a3s/config.acl"]
async fn live_oversized_tool_end_still_projects_event_page() {
    assert!(
        AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES < MAX_OUTPUT_SIZE,
        "suite depends on tool output being able to exceed protocol payload"
    );

    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let large_path = workspace.path().join("large_blob.txt");
    // ~48 KiB body; with line prefixes the retained read output still exceeds
    // the 64 KiB protocol payload bound under the conservative transform.
    let mut body = String::with_capacity(48 * 1024);
    while body.len() < 48 * 1024 {
        body.push_str(LARGE_MARKER);
        body.push('\n');
    }
    std::fs::write(&large_path, &body).expect("plant large file");

    let mut transform = ToolResultTransformPolicyV1::conservative();
    transform.max_output_bytes = MAX_OUTPUT_SIZE;
    transform.head_bytes = MAX_OUTPUT_SIZE;
    transform.tail_bytes = 0;

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("issue-138-page", &["read(**)"])
                    .with_tool_result_transform_policy(transform)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");

    let observed = observe(
        &session,
        "Read large_blob.txt with the read tool (full file). Return a short ack after the tool completes.",
    )
    .await;
    assert!(
        observed.errors.is_empty(),
        "read turn must not fail: {:?}",
        observed.errors
    );
    let read_end = observed.tool_ends.iter().find(|(name, code, output, _)| {
        name == "read" && *code == 0 && output.contains(LARGE_MARKER)
    });
    assert!(
        read_end.is_some(),
        "live read must retain the planted marker: {:?}",
        observed
            .tool_ends
            .iter()
            .map(|(n, c, o, _)| (n.as_str(), *c, o.len()))
            .collect::<Vec<_>>()
    );

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "expected one completed run");
    assert_eq!(runs[0].status, RunStatus::Completed);
    let run_id = runs[0].id.clone();
    let page = session
        .run_event_page(&run_id, None, 64)
        .await
        .expect("run_event_page");
    assert!(!page.events.is_empty(), "run must retain events");

    let has_large_tool_end = page.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentEvent::ToolEnd { name, output, .. }
                if name == "read" && output.len() > AGENT_PROTOCOL_MAX_EVENT_PAYLOAD_BYTES / 2
        )
    });
    assert!(
        has_large_tool_end,
        "expected a large retained tool_end so the protocol bound is exercised; \
         sizes={:?}",
        page.events
            .iter()
            .filter_map(|record| match &record.event {
                AgentEvent::ToolEnd { name, output, .. } => Some((name.as_str(), output.len())),
                _ => None,
            })
            .collect::<Vec<_>>()
    );

    let identity = protocol_identity(session.session_id(), &run_id);
    let observed_at_ms = page
        .events
        .last()
        .map(|record| record.timestamp_ms)
        .unwrap_or(1);
    let projected = AgentProtocolEventPageV1::from_run_page(
        identity,
        RunStatus::Completed,
        observed_at_ms,
        None,
        &page,
    )
    .expect("oversized live tool_end must still project an event page");
    assert!(
        !projected.events.is_empty(),
        "projected page must keep events so hosts can observe terminal state"
    );
    assert!(
        projected
            .events
            .iter()
            .any(|event| event.event.event_type == "tool_end"),
        "projected page must include tool_end"
    );
}

fn progress_mcp_script() -> String {
    format!(
        r#"#!/usr/bin/env python3
import json, sys, time
TOKEN = {token:?}

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if "id" not in msg:
        continue
    mid = msg["id"]
    if method == "initialize":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "progress", "version": "0"}}}}}})
    elif method == "tools/list":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"tools": [{{"name": "lookup", "description": "Return the fixture token after progress notifications.", "inputSchema": {{"type": "object", "properties": {{}}}}, "annotations": {{"readOnlyHint": True, "destructiveHint": False, "openWorldHint": False}}}}]}}}})
    elif method == "tools/call":
        reply({{"jsonrpc": "2.0", "method": "notifications/progress", "params": {{"progressToken": "t", "progress": 1.0, "total": 2.0}}}})
        time.sleep(0.05)
        reply({{"jsonrpc": "2.0", "method": "notifications/progress", "params": {{"progressToken": "t", "progress": 2.0, "total": 2.0}}}})
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"content": [{{"type": "text", "text": TOKEN}}], "isError": False}}}})
    else:
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{}}}})
"#,
        token = MCP_TOKEN
    )
}
