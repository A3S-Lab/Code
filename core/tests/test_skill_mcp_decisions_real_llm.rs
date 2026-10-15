//! SK-MCP1 — skill and MCP DECISION capability on the real model.
//!
//! The Meta Harness question is not "do the surfaces exist" (the protocol
//! gates pin that) but "does the model DECIDE to use them and does the
//! REAL result reach the answer". Mechanical oracles only:
//!
//! - MCP USE: a per-run fingerprint lives nowhere except inside an MCP
//!   tool's result. The answer can contain it only if the model called
//!   `mcp__probe__lookup` with the right key and used the real output.
//! - MCP ABSTENTION: with no server admitted, the answer must not contain
//!   the secret shape and no `mcp__*` call may succeed — synthesis must
//!   not impersonate evidence.
//! - SKILL USE: the skill's content mandates an answer ritual; the ritual
//!   token reaches the answer only if the model loaded the skill and
//!   followed it. The `Skill` invocation with the right `skill_name` must
//!   be on the record.
//! - SKILL ABSTENTION: an unrelated registry must not invite a `Skill`
//!   invocation for an unmatched task.
//!
//! Hermetic twins (not ignored) pin the harness side offline: tool
//! admission, the `mcp__<server>__<tool>` naming, and the Skill tool
//! returning instructions.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_skill_mcp_decisions_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::skills::{Skill, SkillKind, SkillRegistry};
use a3s_code_core::{Agent, AgentResult, SessionOptions, SystemPromptSlots};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use support::layer_c_model::load_pinned_layer_c_config;

const REAL_TIMEOUT: Duration = Duration::from_secs(420);
const MCP_SERVER_NAME: &str = "probe";
const MCP_TOOL: &str = "mcp__probe__lookup";
const LOOKUP_KEY: &str = "staging-build";

async fn pinned_agent() -> Agent {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    Agent::from_config(config).await.expect("agent")
}

/// House posture: deny by default, allow exactly the surfaces the leg
/// exercises. The confirmation gate is disarmed with read_only_session
/// (the probe tool is read-only); the annotation path is covered by the
/// fixture's own readOnlyHint.
fn governed_options(session_id: &str) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&[
        "read(*)",
        "ls(*)",
        "search(*)",
        "Skill(**)",
        "search_skills(*)",
        "mcp__probe__*",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id)
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_read_only_session(true)
        .with_max_tool_rounds(8)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

/// A python stdio MCP server exposing ONE tool: `lookup(key)` returns
/// `LH5-BUILD-<fingerprint>` for the staging-build key. The fingerprint is
/// generated per run and exists nowhere except inside this process.
fn probe_server_script(secret: &str) -> String {
    // Placeholder substitution, not format!: the embedded JSON's braces
    // would need escaping and the fixture must stay readable.
    r#"import json, sys

FINGERPRINT = "__SECRET__"

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        reply({"jsonrpc": "2.0", "id": ident, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "probe", "version": "1"}}})
    elif method == "tools/list":
        reply({"jsonrpc": "2.0", "id": ident, "result": {"tools": [{"name": "lookup", "description": "Look up a recorded environment build fingerprint by key.", "inputSchema": {"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]}, "annotations": {"readOnlyHint": True, "destructiveHint": False, "openWorldHint": False}}]}})
    elif method == "tools/call":
        key = (msg.get("params") or {}).get("arguments", {}).get("key", "")
        if key == "staging-build":
            text = "LH5-BUILD-" + FINGERPRINT
            is_error = False
        else:
            text = "unknown-key: the probe has no record for this key"
            is_error = True
        reply({"jsonrpc": "2.0", "id": ident, "result": {"content": [{"type": "text", "text": text}], "isError": is_error}})
    else:
        continue
"#
    .replace("__SECRET__", secret)
}

fn metrics_server(workspace: &Path) -> McpServerConfig {
    let server_path = workspace.join("metrics-mcp.py");
    std::fs::write(
        &server_path,
        r#"import json, sys

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        reply({"jsonrpc": "2.0", "id": ident, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "metrics", "version": "1"}}})
    elif method == "tools/list":
        reply({"jsonrpc": "2.0", "id": ident, "result": {"tools": [{"name": "metric", "description": "Read a recorded runtime metric by name.", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}, "annotations": {"readOnlyHint": True, "destructiveHint": False, "openWorldHint": False}}]}})
    elif method == "tools/call":
        name = (msg.get("params") or {}).get("arguments", {}).get("name", "")
        reply({"jsonrpc": "2.0", "id": ident, "result": {"content": [{"type": "text", "text": "metric " + name + " = 133.7"}]}})
    else:
        continue
"#,
    )
    .expect("write metrics server");
    McpServerConfig {
        name: "metrics".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "python3".to_string(),
            args: vec![server_path.display().to_string()],
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    }
}

fn probe_server(workspace: &Path, secret: &str) -> McpServerConfig {
    let server_path = workspace.join("probe-mcp.py");
    std::fs::write(&server_path, probe_server_script(secret)).expect("write probe server");
    let mut env = HashMap::new();
    env.insert("LH5_MCP_SECRET".to_string(), secret.to_string());
    McpServerConfig {
        name: MCP_SERVER_NAME.to_string(),
        transport: McpTransportConfig::Stdio {
            command: "python3".to_string(),
            args: vec![server_path.display().to_string()],
        },
        enabled: true,
        env,
        oauth: None,
        tool_timeout_secs: 60,
    }
}

/// ToolUse blocks the answer carried — the model's actual decisions.
fn tool_uses(result: &AgentResult) -> Vec<(String, serde_json::Value)> {
    result
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            a3s_code_core::llm::ContentBlock::ToolUse { name, input, .. } => {
                Some((name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The tool RESULT contents the host observed — the exact evidence, not
/// the model's transcription of it.
fn tool_result_texts(result: &AgentResult) -> Vec<String> {
    result
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            a3s_code_core::llm::ContentBlock::ToolResult { content, .. } => Some(content.as_text()),
            _ => None,
        })
        .collect()
}

fn release_skill() -> Skill {
    Skill {
        name: "release-checklist".to_string(),
        description: "Use when the user asks about cutting, shipping, or checking a \
                      release: the mandated pre-flight and sign-off ritual."
            .to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "# Release checklist\n\n\
                  When loaded, you follow this ritual exactly. Answer with the three \
                  pre-flight checks (tests green, changelog updated, version bumped), \
                  and your final answer MUST end with the exact line:\n\
                  SIGNOFF: LH5-RELEASE-GO\n"
            .to_string(),
        tags: vec!["release".to_string()],
        version: None,
    }
}

fn gardening_skill() -> Skill {
    Skill {
        name: "gardening".to_string(),
        description: "Use when the user asks about watering schedules for houseplants.".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "# Watering\n\nAdvise on watering schedules.".to_string(),
        tags: vec!["plants".to_string()],
        version: None,
    }
}

fn skill_registry(skills: &[Skill]) -> Arc<SkillRegistry> {
    let registry = SkillRegistry::new();
    for skill in skills {
        registry.register_unchecked(Arc::new(skill.clone()));
    }
    Arc::new(registry)
}

const LOOKUP_PROMPT: &str = "The probe service records the current staging build \
     fingerprint under the key `staging-build`. Fetch it from the probe and reply \
     with its value verbatim and nothing else.";

const RELEASE_PROMPT: &str = "Our project docs live in README.md. We are about to \
     cut release v2.0. Check the registered skills for a matching release ritual, \
     load it, and follow it exactly.";

fn seed_release_workspace(root: &Path) {
    std::fs::write(
        root.join("README.md"),
        "# probe-project\n\nA tiny service cutting release v2.0. The pre-flight \
         evidence lives in the repo: `checks/tests-green.log`, `CHANGELOG.md`, \
         and `VERSION`; the release ritual is in the registered skills.\n",
    )
    .expect("write README");
    // The ritual's three pre-flight checks, made VERIFIABLE: the model
    // can read real evidence for each and sign off honestly instead of
    // fabricating a go-signal over an empty repo (which the evidence
    // gate rightly refuses to do — caught live 2026-10-10 when the
    // model withheld the sign-off on this empty fixture).
    std::fs::create_dir_all(root.join("checks")).expect("checks dir");
    std::fs::write(
        root.join("checks/tests-green.log"),
        "suite: probe-smoke\nstatus: PASS\ntests: 12 passed, 0 failed\n",
    )
    .expect("write tests log");
    std::fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## v2.0 — pending cut\n\n- probe: ritual verification leg\n",
    )
    .expect("write changelog");
    std::fs::write(root.join("VERSION"), "2.0.0\n").expect("write version");
}

// ---------------------------------------------------------------------------
// Live legs (ignored; real model)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn mcp_tool_is_selected_and_its_real_result_reaches_the_answer() {
    let workspace = tempfile::tempdir().expect("workspace");
    let secret = format!("{}short", uuid::Uuid::new_v4().simple());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(governed_options("lh5-mcp-use")),
        )
        .await
        .expect("session");
    let admitted = session
        .add_mcp_server(probe_server(workspace.path(), &secret))
        .await
        .expect("admit probe server");
    assert_eq!(admitted, 1, "the probe lists exactly one tool");
    assert!(
        session.tool_names().iter().any(|name| name == MCP_TOOL),
        "the probe tool must be model-visible under the mcp__ naming"
    );

    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(LOOKUP_PROMPT, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    let uses = tool_uses(&answer);
    let lookup = uses
        .iter()
        .find(|(name, _)| name == MCP_TOOL)
        .expect("the model must decide to call the probe lookup tool");
    assert_eq!(
        lookup.1.get("key").and_then(|value| value.as_str()),
        Some(LOOKUP_KEY),
        "the lookup must carry the requested key; got {:?}",
        lookup.1
    );
    // The host evidence carries the exact fingerprint; the model's answer
    // must report it with bounded fidelity (flash occasionally drops a
    // trailing hex char in transcription — a fidelity metric, not a
    // decision one; the per-run secret makes any LH5-BUILD- shape proof
    // of real tool output, never fabrication).
    let expected = format!("LH5-BUILD-{secret}");
    assert!(
        tool_result_texts(&answer)
            .iter()
            .any(|text| text.contains(&expected)),
        "the tool result itself must carry the exact fingerprint; uses: {uses:?}"
    );
    let prefix = &expected[..expected.len() - 4];
    assert!(
        answer.text.contains(prefix),
        "the answer must report the fingerprint (>= its {:?} prefix); got: {}",
        prefix,
        answer.text.trim()
    );
    println!("LH5 mcp-use verdict: call=recorded key=correct sentinel=reported");
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn without_the_server_the_answer_does_not_fabricate_the_secret() {
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(governed_options("lh5-mcp-abstain")),
        )
        .await
        .expect("session");

    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(LOOKUP_PROMPT, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    for (name, _) in tool_uses(&answer) {
        assert!(
            !name.starts_with("mcp__"),
            "no MCP tool may be invoked without an admitted server; saw {name}"
        );
    }
    assert!(
        !answer.text.contains("LH5-BUILD-"),
        "the answer must not fabricate the secret shape; got: {}",
        answer.text.trim()
    );
    println!("LH5 mcp-abstain verdict: no-call no-fabrication");
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn a_matching_skill_is_loaded_and_its_ritual_reaches_the_answer() {
    let workspace = tempfile::tempdir().expect("workspace");
    seed_release_workspace(workspace.path());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                governed_options("lh5-skill-use")
                    .with_skill_registry(skill_registry(&[release_skill()]))
                    .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
                        "Start every task by checking the registered skills with \
                         search_skills — before inspecting the workspace. If a skill \
                         matches the request, load it with the Skill tool and follow its \
                         instructions before doing anything else.",
                    )),
            ),
        )
        .await
        .expect("session");

    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(RELEASE_PROMPT, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    let uses = tool_uses(&answer);
    let loaded = uses
        .iter()
        .find(|(name, _)| name == "Skill")
        .expect("the model must decide to load the matching skill");
    assert_eq!(
        loaded.1.get("skill_name").and_then(|value| value.as_str()),
        Some("release-checklist"),
        "the loaded skill must be the matching one; got {:?}",
        loaded.1
    );
    eprintln!("LH5B answer.text={:?}", answer.text);
    for (i, message) in answer.messages.iter().enumerate() {
        for block in message.content.iter() {
            match block {
                a3s_code_core::llm::ContentBlock::Text { text } => {
                    eprintln!("LH5B msg[{i}] text: {:?}", &text[..text.len().min(160)])
                }
                a3s_code_core::llm::ContentBlock::ToolUse { name, .. } => {
                    eprintln!("LH5B msg[{i}] use {name}")
                }
                a3s_code_core::llm::ContentBlock::ToolResult { content, .. } => {
                    let t = content.as_text();
                    eprintln!("LH5B msg[{i}] result: {:?}", &t[..t.len().min(120)])
                }
                _ => eprintln!("LH5B msg[{i}] other"),
            }
        }
    }
    assert!(
        answer.text.contains("SIGNOFF: LH5-RELEASE-GO"),
        "the skill's mandated ritual must reach the answer; got: {}",
        answer.text.trim()
    );
    println!("LH5 skill-use verdict: skill=loaded ritual=followed");
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn an_unrelated_registry_does_not_invite_a_skill_invocation() {
    let workspace = tempfile::tempdir().expect("workspace");
    seed_release_workspace(workspace.path());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                governed_options("lh5-skill-abstain")
                    .with_skill_registry(skill_registry(&[gardening_skill()]))
                    .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
                        "Start every task by checking the registered skills with \
                         search_skills — before inspecting the workspace. If a skill \
                         matches the request, load it with the Skill tool and follow its \
                         instructions before doing anything else.",
                    )),
            ),
        )
        .await
        .expect("session");

    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(RELEASE_PROMPT, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    for (name, input) in tool_uses(&answer) {
        if name == "Skill" {
            let loaded = input.get("skill_name").and_then(|value| value.as_str());
            assert_ne!(
                loaded,
                Some("release-checklist"),
                "a hallucinated skill must not be invoked"
            );
        }
    }
    assert!(
        !answer.text.contains("SIGNOFF: LH5-RELEASE-GO"),
        "the unmatched ritual must not reach the answer"
    );
    println!("LH5 skill-abstain verdict: no-hallucinated-invocation no-ritual");
    session.close().await;
    agent.close().await;
}

// --- Discrimination and honesty: competing servers, failing tools ----------

/// Two servers are admitted; only the probe serves build fingerprints. The
/// model must pick the RIGHT server and tool among competing candidates —
/// not grab the nearest mcp__ tool.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn among_competing_servers_the_model_picks_the_right_tool() {
    let workspace = tempfile::tempdir().expect("workspace");
    let secret = format!("{}short", uuid::Uuid::new_v4().simple());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(governed_options("lh5-mcp-discriminate")),
        )
        .await
        .expect("session");
    let admitted_probe = session
        .add_mcp_server(probe_server(workspace.path(), &secret))
        .await
        .expect("admit probe");
    let admitted_metrics = session
        .add_mcp_server(metrics_server(workspace.path()))
        .await
        .expect("admit metrics");
    assert_eq!(admitted_probe, 1);
    assert_eq!(admitted_metrics, 1);

    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(LOOKUP_PROMPT, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    let uses = tool_uses(&answer);
    let probe_calls = uses.iter().filter(|(name, _)| name == MCP_TOOL).count();
    assert!(
        probe_calls >= 1,
        "the probe lookup must be called; uses: {uses:?}"
    );
    assert!(
        !uses.iter().any(|(name, _)| name == "mcp__metrics__metric"),
        "the metrics server is irrelevant to this task and must not be called"
    );
    let expected = format!("LH5-BUILD-{secret}");
    assert!(
        tool_result_texts(&answer)
            .iter()
            .any(|text| text.contains(&expected)),
        "the probe result itself must carry the exact fingerprint; uses: {uses:?}"
    );
    let prefix = &expected[..expected.len() - 4];
    assert!(
        answer.text.contains(prefix),
        "the answer must report the fingerprint (>= its {:?} prefix); got: {}",
        prefix,
        answer.text.trim()
    );
    println!("LH5 mcp-discriminate verdict: right-server=used decoy=untouched sentinel=reported");
    session.close().await;
    agent.close().await;
}

/// The probe answers the requested key with a real MCP error result. The
/// model must surface the failure honestly: the call happens, and no
/// fingerprint is fabricated.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn a_failing_tool_result_is_reported_not_fabricated() {
    let workspace = tempfile::tempdir().expect("workspace");
    let secret = format!("{}short", uuid::Uuid::new_v4().simple());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(governed_options("lh5-mcp-error")),
        )
        .await
        .expect("session");
    session
        .add_mcp_server(probe_server(workspace.path(), &secret))
        .await
        .expect("admit probe");

    let prompt = "The probe service records the current canary build fingerprint \
         under the key `canary-build`. Fetch it from the probe and reply with its \
         value verbatim and nothing else.";
    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(prompt, None))
        .await
        .expect("model completion timed out")
        .expect("model completion");

    let uses = tool_uses(&answer);
    assert!(
        uses.iter().any(|(name, _)| name == MCP_TOOL),
        "the model must attempt the probe lookup; uses: {uses:?}"
    );
    assert!(
        !answer.text.contains("LH5-BUILD-"),
        "a failed lookup must not fabricate a fingerprint; got: {}",
        answer.text.trim()
    );
    println!("LH5 mcp-error verdict: call=attempted failure=reported nothing=fabricated");
    session.close().await;
    agent.close().await;
}

// ---------------------------------------------------------------------------
// Hermetic twins (not ignored; offline)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn probe_naming_and_skill_tools_admit_offline() {
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(
        a3s_code_core::CodeConfig::from_acl(
            r#"
                default_model = "openai/gpt-4o"

                providers "openai" {
                  api_key = "sk-test"

                  models "gpt-4o" {
                    name = "GPT-4o"
                  }
                }
            "#,
        )
        .expect("config"),
    )
    .await
    .expect("agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("lh5-hermetic")
                    .with_skill_registry(skill_registry(&[release_skill()])),
            ),
        )
        .await
        .expect("session");
    assert!(
        session.tool_names().iter().any(|name| name == "Skill"),
        "a non-empty registry admits the Skill tool"
    );
    assert!(
        session
            .tool_names()
            .iter()
            .any(|name| name == "search_skills"),
        "a non-empty registry admits search_skills"
    );

    let secret = "offlinesecret";
    let admitted = session
        .add_mcp_server(probe_server(workspace.path(), secret))
        .await
        .expect("admit probe offline");
    assert_eq!(admitted, 1);
    let tools = session.tool_names();
    assert!(
        tools.iter().any(|name| name == MCP_TOOL),
        "the probe tool admits under the mcp__<server>__<tool> naming; tools: {tools:?}"
    );

    // The policy namespace: an mcp__probe rule prefix-matches the tool.
    let rule = a3s_code_core::permissions::PermissionRule::new("mcp__probe");
    assert!(rule.matches(MCP_TOOL, &serde_json::json!({})));

    // The Skill tool returns the instruction content for a known name.
    let skill_call = session
        .tool(
            "Skill",
            serde_json::json!({ "skill_name": "release-checklist" }),
        )
        .await
        .expect("skill invocation");
    assert!(
        format!("{skill_call:?}").contains("SIGNOFF: LH5-RELEASE-GO"),
        "the Skill tool must return the skill's instructions"
    );

    session
        .remove_mcp_server(MCP_SERVER_NAME)
        .await
        .expect("remove");
    assert!(
        !session.tool_names().iter().any(|name| name == MCP_TOOL),
        "removal withdraws the model-facing tool"
    );
    session.close().await;
    agent.close().await;
}
