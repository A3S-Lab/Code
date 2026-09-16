//! Live E2E for the Code-owned Agent Protocol surface (Layer C8).
//!
//! Drives `AgentProtocolHarness` with the pinned Flash model from
//! `.a3s/config.acl`. Pass criteria are protocol kernel effects only:
//! receipt/identity binding, event-page projection, idempotent replay,
//! change-set capture after a live tool mutation, and Cancel of an in-flight
//! run. Assistant wording is never a pass criterion.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_agent_protocol_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentProtocolChangeSetRequestV1, AgentProtocolCommandV1,
    AgentProtocolEventPageRequestV1, AgentProtocolEventPageV1, AgentProtocolHarness,
    AgentProtocolHarnessError, AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1,
    AgentProtocolRunStartV1, AgentProtocolRunStateV1, PlanningMode, SessionOptions,
    AGENT_PROTOCOL_V1,
};
use base64::Engine as _;
use support::layer_c_model::{assert_pinned_layer_c_flash, load_pinned_layer_c_config};

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);
const WRITE_TOKEN: &str = "ap-live-write-token-c8e2";
const WRITE_PATH: &str = "ap-live-protocol.txt";
#[cfg(not(windows))]
const CANCEL_COMMAND: &str =
    "printf STARTED > ap-protocol-cancel-started.txt && sleep 20 && printf LEAK > ap-protocol-cancel-leak.txt";
#[cfg(windows)]
const CANCEL_COMMAND: &str = "[System.IO.File]::WriteAllText('ap-protocol-cancel-started.txt','STARTED'); Start-Sleep -Seconds 20; [System.IO.File]::WriteAllText('ap-protocol-cancel-leak.txt','LEAK')";

fn release_manifest() -> a3s_code_core::release::AgentReleaseManifest {
    a3s_code_core::release::AgentReleaseManifest::parse(include_str!(
        "../../fixtures/agent-release-contract/.a3s/asset.acl"
    ))
    .expect("admit release fixture")
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn initialize_git_workspace(workspace: &std::path::Path) {
    git(workspace, &["init"]);
    git(workspace, &["config", "user.name", "A3S Protocol Live"]);
    git(
        workspace,
        &["config", "user.email", "protocol-live@a3s.invalid"],
    );
    std::fs::write(workspace.join("seed.txt"), "seed\n").expect("write seed");
    git(workspace, &["add", "seed.txt"]);
    git(workspace, &["commit", "-m", "seed"]);
}

async fn live_agent() -> Agent {
    let config = load_pinned_layer_c_config();
    assert_pinned_layer_c_flash(&config, "protocol live suite");
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

fn live_options(allows: &[&str]) -> SessionOptions {
    SessionOptions::new()
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy(allows))
        .with_planning_mode(PlanningMode::Disabled)
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(8)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

fn start_command(
    release_identity: &str,
    session_id: &str,
    run_id: &str,
    prompt: impl Into<String>,
) -> AgentProtocolCommandV1 {
    AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: format!("{run_id}:start"),
            identity: AgentProtocolRunIdentityV1 {
                schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
                protocol: AGENT_PROTOCOL_V1.into(),
                agent_release_identity: release_identity.into(),
                session_id: session_id.into(),
                run_id: run_id.into(),
            },
            prompt: prompt.into(),
        },
    }
}

async fn wait_for_terminal(
    harness: &AgentProtocolHarness,
    command: &AgentProtocolCommandV1,
) -> AgentProtocolEventPageV1 {
    tokio::time::timeout(MODEL_TIMEOUT, async {
        loop {
            let page = harness
                .event_page(&AgentProtocolEventPageRequestV1 {
                    schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
                    identity: command.identity().clone(),
                    after_event_sequence: None,
                    limit: 64,
                })
                .await
                .expect("event page");
            if page.state.is_terminal() {
                break page;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("live protocol run must reach a terminal state")
}

async fn wait_for_change_set(
    harness: &AgentProtocolHarness,
    command: &AgentProtocolCommandV1,
) -> a3s_code_core::AgentProtocolChangeSetV1 {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match harness
                .change_set(&AgentProtocolChangeSetRequestV1 {
                    schema: AgentProtocolChangeSetRequestV1::SCHEMA.into(),
                    identity: command.identity().clone(),
                })
                .await
            {
                Ok(change_set) => break change_set,
                Err(AgentProtocolHarnessError::Host(
                    a3s_code_core::AgentProtocolHostError::ChangeSetPending,
                )) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(error) => panic!("unexpected change-set error: {error}"),
            }
        }
    })
    .await
    .expect("change set must settle after terminal state")
}

fn event_types(page: &AgentProtocolEventPageV1) -> Vec<&str> {
    page.events
        .iter()
        .map(|record| record.event.event_type.as_str())
        .collect()
}

fn non_empty_tool_names(page: &AgentProtocolEventPageV1) -> Vec<String> {
    page.events
        .iter()
        .filter(|record| {
            matches!(
                record.event.event_type.as_str(),
                "tool_start" | "tool_execution_start" | "tool_end"
            )
        })
        .filter_map(|record| {
            record
                .event
                .payload
                .get("name")
                .or_else(|| record.event.payload.get("tool_name"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .collect()
}

/// C8a: Start → receipt → event page → idempotent replay through the Harness.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/deepseek-v4-flash via .a3s/config.acl"]
async fn live_harness_start_projects_events_and_replays() {
    let workspace = tempfile::tempdir().expect("workspace");
    let manifest = release_manifest();
    let release = manifest.artifact().digest().to_string();
    let agent = Arc::new(live_agent().await);
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::clone(&agent),
        workspace.path().display().to_string(),
    )
    .expect("admit harness")
    .with_session_options(live_options(&[]));

    let command = start_command(
        &release,
        "ap-live-start-session",
        "ap-live-start-run-1",
        "Reply with exactly one short sentence confirming you received this protocol start. Do not call tools.",
    );

    let receipt = harness.execute(&command).await.expect("start");
    assert!(!receipt.replayed);
    receipt
        .validate_for(&command)
        .expect("start receipt must bind the command");
    assert_eq!(receipt.identity, *command.identity());
    assert_eq!(receipt.identity.protocol, AGENT_PROTOCOL_V1);

    let page = wait_for_terminal(&harness, &command).await;
    page.validate().expect("terminal page must validate");
    assert_eq!(page.identity, *command.identity());
    assert!(page.state.is_terminal(), "state={:?}", page.state);
    assert!(!page.events.is_empty(), "protocol page must retain events");
    let types = event_types(&page);
    assert!(
        types.iter().any(|t| *t == "agent_end" || *t == "error"),
        "expected agent_end or error in {types:?}"
    );

    let replay = harness.execute(&command).await.expect("replay start");
    assert!(replay.replayed, "exact Start must replay without a new run");
    replay.validate_for(&command).expect("replay receipt");
    let replay_page = wait_for_terminal(&harness, &command).await;
    assert_eq!(replay_page.identity, page.identity);
    assert_eq!(replay_page.state, page.state);

    harness.close().await;
}

/// C8b: Live write through Harness must project tool names and a digest-bound change set.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/deepseek-v4-flash via .a3s/config.acl"]
async fn live_harness_tool_mutation_exports_change_set() {
    let workspace = tempfile::tempdir().expect("workspace");
    initialize_git_workspace(workspace.path());
    let manifest = release_manifest();
    let release = manifest.artifact().digest().to_string();
    let agent = Arc::new(live_agent().await);
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::clone(&agent),
        workspace.path().display().to_string(),
    )
    .expect("admit harness")
    .with_session_options(live_options(&["write(**)"]));

    let command = start_command(
        &release,
        "ap-live-changeset-session",
        "ap-live-changeset-run-1",
        format!(
            "Use the write tool exactly once to create `{WRITE_PATH}` whose entire contents are exactly `{WRITE_TOKEN}` (one line, no extra text). Then stop. Do not invent verification."
        ),
    );

    let receipt = harness.execute(&command).await.expect("start write run");
    assert!(!receipt.replayed);
    receipt.validate_for(&command).expect("write start receipt");

    let page = wait_for_terminal(&harness, &command).await;
    page.validate().expect("write terminal page");
    assert!(page.state.is_terminal(), "state={:?}", page.state);
    let tool_names = non_empty_tool_names(&page);
    assert!(
        tool_names.iter().any(|name| name == "write"),
        "protocol page must project a non-empty write tool name; got {tool_names:?} types={:?}",
        event_types(&page)
    );

    let change_set = wait_for_change_set(&harness, &command).await;
    change_set.validate().expect("change set must validate");
    assert_eq!(change_set.identity, *command.identity());
    assert_eq!(change_set.state, page.state);
    let patch = base64::engine::general_purpose::STANDARD
        .decode(&change_set.patch_base64)
        .expect("decode patch");
    let patch = String::from_utf8(patch).expect("utf8 patch");
    assert!(
        patch.contains(WRITE_PATH) || patch.contains(WRITE_TOKEN),
        "change set must capture the live write; patch={patch}"
    );

    harness.close().await;
}

/// C8c: Protocol Cancel must terminate an in-flight live tool without late effects.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/deepseek-v4-flash via .a3s/config.acl"]
async fn live_harness_cancel_stops_in_flight_tool() {
    let workspace = tempfile::tempdir().expect("workspace");
    let manifest = release_manifest();
    let release = manifest.artifact().digest().to_string();
    let agent = Arc::new(live_agent().await);
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::clone(&agent),
        workspace.path().display().to_string(),
    )
    .expect("admit harness")
    .with_session_options(live_options(&[&format!("bash({CANCEL_COMMAND})")]));

    let command = start_command(
        &release,
        "ap-live-cancel-session",
        "ap-live-cancel-run-1",
        format!(
            "Invoke bash exactly once with this command: `{CANCEL_COMMAND}`. Do not substitute another command."
        ),
    );

    harness.execute(&command).await.expect("start cancel run");

    tokio::time::timeout(MODEL_TIMEOUT, async {
        loop {
            let page = harness
                .event_page(&AgentProtocolEventPageRequestV1 {
                    schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
                    identity: command.identity().clone(),
                    after_event_sequence: None,
                    limit: 64,
                })
                .await
                .expect("event page while waiting for tool");
            if page.state.is_terminal() {
                panic!(
                    "run reached {:?} before Cancel; types={:?}",
                    page.state,
                    event_types(&page)
                );
            }
            let started = page.events.iter().any(|record| {
                matches!(
                    record.event.event_type.as_str(),
                    "tool_start" | "tool_execution_start"
                )
            }) || workspace
                .path()
                .join("ap-protocol-cancel-started.txt")
                .exists();
            if started {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("live tool must begin before Cancel");

    let cancel = AgentProtocolCommandV1::Cancel {
        request: AgentProtocolRunCancelV1 {
            schema: AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "ap-live-cancel-run-1:cancel".into(),
            identity: command.identity().clone(),
            reason: "protocol_live_e2e".into(),
        },
    };
    let cancelled = harness.execute(&cancel).await.expect("cancel");
    cancelled
        .validate_for(&cancel)
        .expect("cancel receipt must bind");
    assert_eq!(cancelled.state, AgentProtocolRunStateV1::Cancelled);

    let page = wait_for_terminal(&harness, &command).await;
    assert_eq!(page.state, AgentProtocolRunStateV1::Cancelled);
    assert!(
        !workspace
            .path()
            .join("ap-protocol-cancel-leak.txt")
            .exists(),
        "cancelled bash must not write the late leak file"
    );

    let replay = harness.execute(&cancel).await.expect("cancel replay");
    assert!(replay.replayed);
    replay.validate_for(&cancel).expect("cancel replay receipt");

    harness.close().await;
}
