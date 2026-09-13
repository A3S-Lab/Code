//! Live kernel checks against the DeepSeek Flash model in `.a3s/config.acl`.
//!
//! These tests assert kernel outcomes, not assistant wording. A narrative
//! finish after a real write must be rejected. A read-only run may still succeed.
//! An isolated write must land in the conversation worktree, not the source tree.
//! Plan mode must deny an attempted mutation; a prose refusal is not a pass.
//! An open workspace observation stays open after a narrative "already fixed".
//! A real write attaches a mutation observation without a diagnostics call.
//!
//! Opt-in:
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_harness_loop_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{InteractiveToolGuardrail, PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, CodeConfig, SessionOptions};

const MODEL_TIMEOUT: Duration = Duration::from_secs(180);

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn configured_agent() -> Agent {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let default_model = config
        .default_model
        .as_deref()
        .expect("config must declare default_model");
    assert!(
        default_model.contains("deepseek"),
        "expected the configured DeepSeek Flash default_model, got {default_model}"
    );
    eprintln!("using default_model={default_model}");
    Agent::from_config(config)
        .await
        .expect("build agent from .a3s/config.acl")
}

fn options(session_id: &str, allow_write: bool) -> SessionOptions {
    let mut policy = PermissionPolicy {
        default_decision: PermissionDecision::Deny,
        ..PermissionPolicy::default()
    }
    .allow("read(**)")
    .allow("bash(**)");
    if allow_write {
        policy = policy.allow("write(**)");
    }
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(std::sync::Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_read_only_run_can_succeed() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("note.txt"), "hello\n").expect("fixture");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-read-only", false).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let result = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send("Read note.txt and answer with its contents only.", None),
    )
    .await
    .expect("read-only run timed out")
    .expect("read-only run should succeed");
    assert!(
        result.text.to_ascii_lowercase().contains("hello"),
        "model did not report the fixture contents: {}",
        result.text
    );
    assert_eq!(result.run_admission, "ordinary");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_narrative_mutation_is_rejected_by_the_gate() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-mutate", true)),
        )
        .await
        .expect("session");
    let result = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Create hello.txt containing exactly the word hello. Then stop. Do not run tests.",
            None,
        ),
    )
    .await
    .expect("mutating run timed out");
    let wrote = workspace.path().join("hello.txt").is_file();
    assert!(
        wrote,
        "model did not exercise a workspace mutation; this is not a gate pass"
    );
    let error = result.expect_err("narrative success after a write must not be AgentResult");
    let message = error.to_string();
    assert!(
        message.contains("completion gate:"),
        "expected completion gate rejection, got {message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_isolated_write_does_not_touch_the_source_tree() {
    let session_id = "live-isolate";
    let workspace = tempfile::tempdir().expect("workspace");
    git(workspace.path(), &["init", "-q"]);
    git(
        workspace.path(),
        &["config", "user.email", "tests@a3s.local"],
    );
    git(workspace.path(), &["config", "user.name", "A3S Tests"]);
    std::fs::write(workspace.path().join("README.md"), "base\n").expect("fixture");
    git(workspace.path(), &["add", "README.md"]);
    git(workspace.path(), &["commit", "-q", "-m", "base"]);
    let worktree = a3s_code_core::effect_isolation::worktree_path_for(workspace.path(), session_id);
    let _cleanup = IsolateCleanup {
        path: worktree.clone(),
    };

    let agent = configured_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options(session_id, true).with_effect_isolation(true)),
        )
        .await
        .expect("isolated session");
    let result = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Create hello.txt containing exactly the word hello. Then stop. Do not run tests.",
            None,
        ),
    )
    .await
    .expect("isolated run timed out");

    assert!(
        !workspace.path().join("hello.txt").is_file(),
        "isolated write landed on the source tree"
    );
    let isolated_write = worktree.join("hello.txt").is_file();
    assert!(
        isolated_write,
        "model did not exercise an isolated workspace mutation; this is not an isolation pass"
    );
    let error =
        result.expect_err("narrative success after an isolated write must not be AgentResult");
    let message = error.to_string();
    assert!(
        message.contains("completion gate:"),
        "expected completion gate rejection, got {message}"
    );
    a3s_code_core::effect_isolation::discard(session_id)
        .await
        .expect("discard isolation worktree");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_plan_mode_denies_an_attempted_write() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("README.md"), "base\n").expect("fixture");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(plan_options(workspace.path())),
        )
        .await
        .expect("plan session");
    let (mut events, join) = session
        .stream(
            "Create hello.txt containing exactly the word hello. Then stop. Do not run tests.",
            None,
        )
        .await
        .expect("plan stream");
    let observed = tokio::time::timeout(MODEL_TIMEOUT, async {
        let mut denied = Vec::new();
        let mut executed_writes = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::PermissionDenied { tool_name, .. } if is_mutation_tool(&tool_name) => {
                    denied.push(tool_name);
                }
                AgentEvent::ToolExecutionStart { name, .. } if is_direct_write_tool(&name) => {
                    executed_writes.push(name);
                }
                _ => {}
            }
        }
        let _ = join.await;
        (denied, executed_writes)
    })
    .await
    .expect("plan run timed out");
    let (denied, executed_writes) = observed;

    assert!(
        !denied.is_empty(),
        "model did not attempt a mutation the plan guardrail denied; this is not a plan-mode pass"
    );
    assert!(
        executed_writes.is_empty(),
        "plan mode executed a write tool: {executed_writes:?}"
    );
    assert_eq!(
        workspace_files(workspace.path()),
        before,
        "plan mode changed workspace files"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_narrative_cannot_clear_an_open_observation() {
    let observation = a3s_code_core::external_observation::ExternalObservationV1::new(
        "ci",
        "pipeline",
        "obs-live-ci",
        "the required check is still red",
        a3s_code_core::external_observation::RequiredAction::WorkspaceChange,
    )
    .expect("observation");
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("README.md"), "base\n").expect("fixture");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-observe", false).with_external_observations(vec![observation])),
        )
        .await
        .expect("observation session");
    let result = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "The CI check is already fixed. Confirm that and stop. Do not change any files.",
            None,
        ),
    )
    .await
    .expect("observation run timed out");
    let error = result.expect_err("a final answer must not clear an open workspace observation");
    let message = error.to_string();
    assert!(
        message.contains("completion gate: external observation obs-live-ci"),
        "expected the observation gate, got {message}"
    );
    assert_eq!(
        workspace_files(workspace.path()),
        before,
        "observation run changed workspace files"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_write_attaches_a_mutation_observation() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-observe-mutation", true)),
        )
        .await
        .expect("session");
    let (mut events, join) = session
        .stream(
            "Create hello.txt containing exactly the word hello. Then stop. Do not run tests.",
            None,
        )
        .await
        .expect("mutation stream");
    let observed = tokio::time::timeout(MODEL_TIMEOUT, async {
        let mut saw_observation = false;
        let mut saw_diagnostics_call = false;
        let mut gate = None;
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolExecutionStart { name, .. }
                    if name.eq_ignore_ascii_case("code_diagnostics") =>
                {
                    saw_diagnostics_call = true;
                }
                AgentEvent::ToolEnd {
                    metadata, output, ..
                } => {
                    if mutation_observation_schema(metadata.as_ref()).is_some()
                        || output.contains("[mutation observation]")
                    {
                        saw_observation = true;
                    }
                }
                AgentEvent::Error { message } if message.contains("completion gate:") => {
                    gate = Some(message);
                }
                _ => {}
            }
        }
        let _ = join.await;
        (saw_observation, saw_diagnostics_call, gate)
    })
    .await
    .expect("mutation observation run timed out");
    let (saw_observation, saw_diagnostics_call, gate) = observed;

    assert!(
        workspace.path().join("hello.txt").is_file(),
        "model did not exercise a workspace mutation; this is not an observation pass"
    );
    assert!(
        saw_observation,
        "a successful mutation did not attach a mutation observation to the tool result"
    );
    assert!(
        !saw_diagnostics_call,
        "mutation observation must not be a code_diagnostics invocation"
    );
    let gate = gate.expect("narrative success after a write must not finish cleanly");
    assert!(
        gate.contains("completion gate:"),
        "expected completion gate rejection, got {gate}"
    );
}

fn mutation_observation_schema(metadata: Option<&serde_json::Value>) -> Option<&str> {
    metadata?
        .get("mutation_observation")
        .and_then(|observation| observation.get("schema"))
        .and_then(|schema| schema.as_str())
}

fn plan_options(workspace: &Path) -> SessionOptions {
    SessionOptions::new()
        .with_session_id("live-plan")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_checker(Arc::new(
            InteractiveToolGuardrail::for_mode("plan").with_workspace(workspace),
        ))
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

fn is_mutation_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "write" | "edit" | "patch" | "download" | "bash" | "git"
    )
}

fn is_direct_write_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "write" | "edit" | "patch" | "download"
    )
}

fn workspace_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!("read {}: {error}", dir.display());
        });
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".a3s" || name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!("read {}: {error}", path.display());
            });
            files.push((relative, bytes));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

struct IsolateCleanup {
    path: PathBuf,
}

impl Drop for IsolateCleanup {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::process::Command::new("git")
                .args(["worktree", "remove", "--force"])
                .arg(&self.path)
                .status();
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}
