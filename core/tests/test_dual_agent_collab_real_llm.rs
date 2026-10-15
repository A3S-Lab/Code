//! Live dual-agent collaboration gates against `.a3s/config.acl`.
//!
//! Three product contracts over the main-agent + reviewer-agent architecture:
//!
//! 1. **Model-driven review delegation** — the real main model chooses to
//!    delegate a code review to the built-in `review` subagent via the `task`
//!    tool, and reports the reviewer's findings (a planted bug must surface).
//! 2. **Provenance chain** — the same real turn emits digest-bound harness
//!    evidence (`ToolRequestBound` with argument digests), and its captured
//!    events form a complete, digest-addressed evidence snapshot.
//! 3. **Skill × MCP composition** — a skill whose allowed-tools name an MCP
//!    tool runs as a child agent that actually calls the attached MCP server;
//!    the parent answers with the server's token. This combination (skill
//!    allowlist ∘ parent checker ∘ MCP annotations) has no coverage anywhere
//!    else in the tree.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_dual_agent_collab_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::{Agent, AgentEvent, SessionOptions, SystemPromptSlots};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

const TURN_TIMEOUT: Duration = Duration::from_secs(600);
const REVIEW_MARKER: &str = "BUG-LIVE-REVIEW-7741";
const RELAY_TOKEN: &str = "RELAY-TOKEN-3fd1";

#[derive(Default)]
struct TurnRecord {
    tool_request_binds: Vec<(String, String, String)>,
    subagent_starts: Vec<String>,
    subagent_progress: usize,
    subagent_ends: Vec<(String, bool)>,
    tool_calls: Vec<(String, bool)>,
    confirmations: Vec<String>,
    final_text: String,
}

async fn run_turn(session: &a3s_code_core::AgentSession, prompt: &str) -> TurnRecord {
    let (mut events, worker) = session
        .stream(prompt, None)
        .await
        .expect("start model turn");
    let mut record = TurnRecord::default();
    let mut pending_tools: HashMap<String, (String, bool)> = HashMap::new();
    let final_text = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await.expect("event stream open") {
                AgentEvent::ToolRequestBound {
                    tool_id,
                    tool_name,
                    snapshot,
                } => {
                    record.tool_request_binds.push((
                        tool_id,
                        tool_name,
                        snapshot.arguments_digest.to_string(),
                    ));
                }
                AgentEvent::ToolExecutionStart { id, name, .. } => {
                    pending_tools.insert(id, (name, false));
                }
                AgentEvent::ToolEnd { id, exit_code, .. } => {
                    if let Some((name, _)) = pending_tools.remove(&id) {
                        record.tool_calls.push((name, exit_code == 0));
                    }
                }
                AgentEvent::SubagentStart { agent, .. } => {
                    record.subagent_starts.push(agent);
                }
                AgentEvent::SubagentProgress { status, .. } => {
                    record.subagent_progress += 1;
                    let _ = status;
                }
                AgentEvent::SubagentEnd { success, .. } => {
                    record.subagent_ends.push((String::new(), success));
                }
                AgentEvent::End { text, .. } => break text,
                AgentEvent::Error { message } => panic!("model turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    // AutoApprove sessions still surface the escalation event
                    // as telemetry (child-confirmed tools route to the parent
                    // UI); record it, do not treat it as a failure.
                    record.confirmations.push(tool_name);
                }
                _ => {}
            }
        }
    })
    .await
    .expect("model turn timed out");
    worker.await.expect("worker joins");
    record.final_text = final_text;
    record
}

fn write_review_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("src/budget.rs"),
        format!(
            r#"//! Billing budget guard.
pub const MAX_LINES: usize = 10;

/// Sum the billed lines for one invoice. {REVIEW_MARKER}: the bound is
/// exclusive on the wrong side, so an invoice with exactly MAX_LINES lines
/// is rejected.
pub fn billed_lines(lines: &[u8]) -> usize {{
    let mut total = 0usize;
    for index in 0..=MAX_LINES {{
        if let Some(line) = lines.get(index) {{
            total += usize::from(*line);
        }}
    }}
    total
}}
"#
        ),
    )
    .expect("write review fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_delegates_review_and_the_provenance_chain_binds() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    config.default_model = Some(model.clone());
    let agent = Agent::from_config(config)
        .await
        .expect("agent from real config");

    let workspace = tempfile::tempdir().expect("workspace");
    write_review_fixture(workspace.path());

    let mut permissions = a3s_code_core::permissions::PermissionPolicy::new()
        .allow_all(&["task", "read", "ls", "grep", "glob", "Skill"]);
    permissions.default_decision = a3s_code_core::permissions::PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id("dual-agent-review")
        .with_model(model.clone())
        .with_permission_policy(permissions)
        .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
        .with_planning(false)
        .with_manual_delegation_enabled(true)
        .with_auto_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(8)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "Delegate the code review to the review subagent with the task tool \
             (agent \"review\") before answering. The reviewer must read \
             src/budget.rs itself. Report the reviewer's concrete findings.",
        ));
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("review session");

    let record = run_turn(
        &session,
        "Review src/budget.rs for correctness by delegating to the review \
         subagent, then summarize the reviewer's findings. Name the exact \
         defect the reviewer identified.",
    )
    .await;

    // Dual-agent contract: the main model actually delegated to review…
    assert!(
        record
            .tool_calls
            .iter()
            .any(|(name, ok)| name == "task" && *ok),
        "the main model must call the task tool: {:?}",
        record.tool_calls
    );
    // …and the subagent lifecycle reached the host channel. Which lifecycle
    // variants the task path emits varies (parallel fan-out pre-emits
    // starts; foreground single tasks emit progress), so the lane contract
    // is: at least one subagent telemetry event arrives on the stream.
    let lifecycle_events =
        record.subagent_starts.len() + record.subagent_progress + record.subagent_ends.len();
    assert!(
        lifecycle_events > 0,
        "subagent telemetry must reach the host channel: starts={:?} progress={} \
         ends={:?} tools={:?}",
        record.subagent_starts,
        record.subagent_progress,
        record.subagent_ends,
        record.tool_calls
    );

    // The parent must ground its summary in the reviewer's finding — the
    // planted defect is only visible by reading the file.
    let answer = record.final_text.to_lowercase();
    assert!(
        answer.contains("off") && (answer.contains("bound") || answer.contains("max_lines")),
        "the parent answer must carry the reviewer's concrete finding: {:?}",
        record.final_text
    );

    // Provenance contract: the turn emitted digest-bound tool-request
    // evidence for the delegation call itself.
    let task_bind = record
        .tool_request_binds
        .iter()
        .find(|(_, name, _)| name == "task")
        .expect("the delegation call must carry a ToolRequestBound snapshot");
    assert!(
        task_bind.2.starts_with("sha256:"),
        "the delegation arguments must be digest-bound: {:?}",
        task_bind
    );

    session.close().await;
    println!(
        "dual-agent-review-real-llm model={} subagents={:?} task_bind_digest={} \
         tool_binds={} total_ms={} result=pass",
        model,
        record.subagent_starts,
        &task_bind.2[..19],
        record.tool_request_binds.len(),
        started.elapsed().as_millis()
    );
}

fn write_relay_skill(root: &std::path::Path) {
    let skill_dir = root.join("skills").join("mcp-relay");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: mcp-relay\ndescription: Fetch the relay token from the \
         attached relay MCP server and report it verbatim.\nallowed-tools: \
         mcp__relay__echo_token\n---\nCall the mcp__relay__echo_token tool \
         with an empty object and report the returned text verbatim.",
    )
    .expect("write skill");
}

fn relay_server_config(script: &std::path::Path, log: &std::path::Path) -> McpServerConfig {
    McpServerConfig {
        name: "relay".to_owned(),
        transport: McpTransportConfig::Stdio {
            command: support::python_stdio_command(),
            args: vec![script.display().to_string()],
        },
        enabled: true,
        env: HashMap::from([("A3S_RELAY_LOG".to_owned(), log.display().to_string())]),
        oauth: None,
        tool_timeout_secs: 60,
    }
}

fn relay_script() -> String {
    r#"import json, os, sys

log_path = os.environ.get("A3S_RELAY_LOG")
def log(event):
    if log_path:
        with open(log_path, "a", encoding="ascii") as handle:
            handle.write(event + "\n")

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
        reply({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "relay", "version": "0"}}})
    elif method == "tools/list":
        reply({"jsonrpc": "2.0", "id": mid, "result": {"tools": [{"name": "echo_token", "description": "Return the relay token.", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        log("tools_call:" + str(msg.get("params", {}).get("name", "")))
        reply({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "RELAY-TOKEN-3fd1"}], "isError": False}})
    else:
        reply({"jsonrpc": "2.0", "id": mid, "result": {}})
"#
    .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials, python3, and network access"]
async fn real_model_uses_a_skill_whose_child_calls_the_mcp_server() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    config.default_model = Some(model.clone());
    let agent = Agent::from_config(config)
        .await
        .expect("agent from real config");

    let workspace = tempfile::tempdir().expect("workspace");
    write_relay_skill(workspace.path());
    let script = workspace.path().join("relay_mcp_fixture.py");
    std::fs::write(&script, relay_script()).expect("write fixture script");
    // Keep the relay log OUTSIDE the workspace: the porcelain mutation
    // ledger correctly attributes any in-workspace write to the session and
    // the completion gate refuses to settle an unverified mutation.
    let outside = tempfile::tempdir().expect("outside dir");
    let relay_log = outside.path().join("relay_calls.log");
    std::fs::write(&relay_log, "").expect("clear relay log");

    let options = SessionOptions::new()
        .with_session_id("skill-mcp-relay")
        .with_model(model.clone())
        .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(6)
        .with_skill_dirs(vec![workspace.path().join("skills")]);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("skill-mcp session");
    let published = session
        .add_mcp_server(relay_server_config(&script, &relay_log))
        .await
        .expect("attach relay MCP server");
    assert_eq!(published, 1, "the echo_token tool must publish");

    let record = run_turn(
        &session,
        "Use the mcp-relay skill to fetch the relay token, then report the \
         token exactly as the skill returns it.",
    )
    .await;

    // Skill execution contract: the parent invoked the Skill tool and a
    // child agent ran it (allowed-tools declared → child path).
    assert!(
        record
            .tool_calls
            .iter()
            .any(|(name, ok)| name == "Skill" && *ok),
        "the parent must invoke the Skill tool: {:?}",
        record.tool_calls
    );
    // The MCP tool carries no safety annotations, so its call must be
    // confirmation-escalated — and the child routes that escalation to the
    // parent UI through the bridged lane.
    assert!(
        record
            .confirmations
            .iter()
            .any(|tool| tool == "mcp__relay__echo_token"),
        "the MCP tool call must escalate to the parent UI: {:?}",
        record.confirmations
    );

    // Server-side ground truth: the child's MCP call reached the server.
    // (Child tool events intentionally stay off the parent stream; the
    // relay fixture logs every tools/call it serves.)
    let relay_calls = std::fs::read_to_string(&relay_log).unwrap_or_default();
    assert!(
        relay_calls.contains("tools_call:echo_token"),
        "the skill child must call the MCP tool server-side: {relay_calls:?}"
    );
    assert!(
        record.final_text.contains(RELAY_TOKEN),
        "the parent answer must carry the MCP-served token: {:?}",
        record.final_text
    );

    session.close().await;
    println!(
        "skill-mcp-real-llm model={} tools={:?} total_ms={} result=pass",
        model,
        record
            .tool_calls
            .iter()
            .map(|(name, ok)| format!("{name}{}", if *ok { "" } else { "!" }))
            .collect::<Vec<_>>(),
        started.elapsed().as_millis()
    );
}
