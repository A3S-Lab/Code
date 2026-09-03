//! Live-provider qualification for a bounded, multi-step coding task.
//!
//! The test requires the selected model to plan, establish a failing baseline,
//! inspect a specification and implementation, edit two source files, and
//! verify the repair. It is ignored by default because it consumes provider
//! quota. Select a model declared in the same ACL with
//! `A3S_TEST_MODEL=provider/model`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, PlanningMode, RunStatus, SessionOptions,
    SystemPromptSlots,
};

const REAL_TIMEOUT: Duration = Duration::from_secs(420);
const SPEC: &str = r#"# Label summary contract

`normalizeLabels(values)` must trim every string, lowercase it, discard empty
values, remove duplicates, and return the remaining labels in ascending order.

`summarize(values)` must return `{ labels, uniqueCount }`, where `labels` is the
normalized list and `uniqueCount` is exactly its length.
"#;
const TEST_SOURCE: &str = r#"import assert from "node:assert/strict";
import { summarize } from "./src/stats.mjs";

const actual = summarize([" Beta ", "alpha", "ALPHA", "", "beta", "Gamma"]);
assert.deepEqual(actual, {
  labels: ["alpha", "beta", "gamma"],
  uniqueCount: 3,
});
console.log("LONG_HORIZON_OK");
"#;
const NORMALIZE_BUG: &str = r#"export function normalizeLabels(values) {
  return values.map((value) => value.trim());
}
"#;
const STATS_BUG: &str = r#"import { normalizeLabels } from "./normalize.mjs";

export function summarize(values) {
  return { labels: normalizeLabels(values), uniqueCount: 99 };
}
"#;

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn configured_agent_and_model() -> (Agent, String) {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let model = std::env::var("A3S_TEST_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config.default_model.clone())
        .expect("real config must declare default_model");
    let (provider, model_id) = model
        .split_once('/')
        .expect("selected model must use provider/model syntax");
    assert!(
        config.llm_config(provider, model_id).is_some(),
        "selected model {model} is not declared in {}",
        path.display()
    );
    eprintln!("[long-horizon-real] model={model}");
    (
        Agent::from_config(config)
            .await
            .expect("build real-provider agent"),
        model,
    )
}

fn session_options(model: &str) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&[
        "ls(*)",
        "read(*)",
        "search(*)",
        "edit(src/**)",
        "write(src/**)",
        "patch(*)",
        "bash(node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(format!("long-horizon-{}", model.replace('/', "-")))
        .with_model(model)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(PlanningMode::Enabled)
        .with_goal_tracking(true)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(16)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "This is a coding conformance task. Keep a concise plan, gather direct evidence, preserve the specification and test, make only necessary source edits, and do not claim success until the exact test command passes.",
        ))
}

fn seed_workspace(root: &Path) {
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("SPEC.md"), SPEC).unwrap();
    std::fs::write(root.join("test.mjs"), TEST_SOURCE).unwrap();
    std::fs::write(root.join("src/normalize.mjs"), NORMALIZE_BUG).unwrap();
    std::fs::write(root.join("src/stats.mjs"), STATS_BUG).unwrap();
}

async fn run_task(session: &AgentSession) -> a3s_code_core::AgentResult {
    tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            r#"Repair this small JavaScript project by following every step:
1. Run exactly `node test.mjs` before editing and observe the failure.
2. Read SPEC.md, test.mjs, src/normalize.mjs, and src/stats.mjs.
3. Fix the implementation in both source files. Do not modify SPEC.md or test.mjs and do not create dependencies.
4. Run exactly `node test.mjs` again and require it to print LONG_HORIZON_OK.
5. Review the resulting source against SPEC.md, then reply with exactly LONG_HORIZON_COMPLETE and no other text."#,
            None,
        ),
    )
    .await
    .expect("long-horizon scenario timed out")
    .expect("long-horizon scenario failed")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_completes_evidence_gated_multi_step_coding_task() {
    let workspace = tempfile::tempdir().expect("long-horizon workspace");
    seed_workspace(workspace.path());
    let (agent, model) = configured_agent_and_model().await;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(&model)),
        )
        .await
        .expect("create long-horizon session");

    let result = run_task(&session).await;
    assert_eq!(result.text.trim(), "LONG_HORIZON_COMPLETE");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("SPEC.md")).unwrap(),
        SPEC
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("test.mjs")).unwrap(),
        TEST_SOURCE
    );
    assert_ne!(
        std::fs::read_to_string(workspace.path().join("src/normalize.mjs")).unwrap(),
        NORMALIZE_BUG
    );
    assert_ne!(
        std::fs::read_to_string(workspace.path().join("src/stats.mjs")).unwrap(),
        STATS_BUG
    );

    let verification = tokio::process::Command::new("node")
        .arg("test.mjs")
        .current_dir(workspace.path())
        .stdin(Stdio::null())
        .output()
        .await
        .expect("run independent verification");
    assert!(
        verification.status.success(),
        "independent test failed: {}{}",
        String::from_utf8_lossy(&verification.stdout),
        String::from_utf8_lossy(&verification.stderr)
    );
    assert!(String::from_utf8_lossy(&verification.stdout).contains("LONG_HORIZON_OK"));

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Completed);
    let events = session.run_events(&runs[0].id).await;
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::PlanningStart { .. })));
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::PlanningEnd { .. })));
    let bash_ends = events
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::ToolEnd {
                name, exit_code, ..
            } if name == "bash" => Some(*exit_code),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        bash_ends.iter().any(|code| *code != 0),
        "missing failing baseline: {bash_ends:?}"
    );
    assert!(
        bash_ends.contains(&0),
        "missing successful verification: {bash_ends:?}"
    );
    assert!(events.iter().any(|record| matches!(
        &record.event,
        AgentEvent::ToolEnd { name, exit_code: 0, .. } if name == "read"
    )));
    assert!(events.iter().any(|record| matches!(
        &record.event,
        AgentEvent::ToolEnd { name, exit_code: 0, .. }
            if matches!(name.as_str(), "edit" | "patch" | "write")
    )));
    // A denied exploratory request is not a harness failure: the authority
    // boundary must contain it and the agent may recover by using the exact
    // permitted operation. The immutable spec/test assertions and independent
    // verification above prove that no denied mutation escaped.
    let denied = events
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::PermissionDenied {
                tool_name, reason, ..
            } => Some(format!("{tool_name}: {reason}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !denied.is_empty() {
        eprintln!("[long-horizon-real] contained denied requests: {denied:?}");
    }
}
