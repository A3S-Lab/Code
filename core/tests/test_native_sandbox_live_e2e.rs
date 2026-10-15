#![cfg(test)]

//! Native admission and live E2E for Code's resolved a3s-sandbox dependency.
//! The current manifest pins 0.2.2@7e948a36 and the workspace patches it to
//! the local Sandbox tree. Neither a version label nor a Git declaration
//! proves execution qualification or public-source parity; retain the actual
//! resolved package, source and test-binary identities with each campaign.
//!
//! First principles:
//! - Integration means the default local session path uses `NativeBashSandbox`
//!   (Seatbelt / namespaces / AppContainer), not process-host fallback.
//! - macOS native execution is refused before spawn while descendant lifetime
//!   is unqualified. Its provider-free refusal test does not qualify native
//!   execution, isolation, containment, or the live leg.
//! - On other platforms the existing supported-execution assertions remain:
//!   probe, sandboxed tool metadata, on-disk effects, and outside-workspace
//!   denial. This slice supplies no new platform qualification.
//! - The mandatory ignored live test remains discoverable on macOS and fails
//!   its lifetime preflight before configuration/provider calls. Refusal is
//!   never substituted for a live support pass. Assistant wording is no oracle.
//! - Model pin comes from monorepo `.a3s/config.acl` Layer C Flash
//!   (`boyue/deepseek-v4-flash`; alternate route
//!   `boyue/bailian/deepseek-v4.1-flash` remains for model-switch E2E).
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/to/a3s/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_native_sandbox_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::sandbox::native::{NativeBashSandbox, NATIVE_SANDBOX_BACKEND};
use a3s_code_core::sandbox::{BashSandbox, SandboxCommandRequest};
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use support::layer_c_model::{assert_pinned_layer_c_flash, load_pinned_layer_c_config};

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);
const NATIVE_TOKEN: &str = "native-sandbox-live-token-0a3s";
#[cfg(target_os = "macos")]
const NATIVE_LIFETIME_UNQUALIFIED: &str = "native_process_lifetime_unqualified";

fn assert_live_native_lifetime_supported() {
    #[cfg(target_os = "macos")]
    panic!(
        "unsupported: {NATIVE_LIFETIME_UNQUALIFIED}; macOS native foreground/background lifetime is unqualified; mandatory live support remains blocked before config/provider initialization"
    );
}

async fn configured_agent() -> Agent {
    let config = load_pinned_layer_c_config();
    assert_pinned_layer_c_flash(&config, "live native-sandbox suite");
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
        policy = policy.allow(rule);
    }
    policy
}

fn options(session_id: &str, allows: &[&str]) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy(allows))
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

struct Observed {
    tools: Vec<(String, i32, String, Option<serde_json::Value>)>,
    errors: Vec<String>,
    end_text: String,
}

async fn observe(session: &a3s_code_core::AgentSession, prompt: &str) -> Observed {
    let (mut events, join) = session.stream(prompt, None).await.expect("stream");
    let observed = tokio::time::timeout(MODEL_TIMEOUT, async {
        let mut observed = Observed {
            tools: Vec::new(),
            errors: Vec::new(),
            end_text: String::new(),
        };
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolEnd {
                    name,
                    exit_code,
                    output,
                    metadata,
                    ..
                } => {
                    observed.tools.push((name, exit_code, output, metadata));
                }
                AgentEvent::Error { message, .. } => observed.errors.push(message),
                AgentEvent::End { text, .. } => observed.end_text = text,
                _ => {}
            }
        }
        observed
    })
    .await
    .expect("model stream timed out");
    let _ = join.await;
    observed
}

/// Actual native refusal, not a supported execution or containment proof.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn native_macos_refuses_before_workspace_or_outside_writes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside_root = tempfile::tempdir().expect("owned outside-workspace sentinel root");
    let outside = outside_root.path().join("native-refusal-outside.txt");
    let nonce = format!("native-refusal-{}", uuid::Uuid::new_v4());
    let sandbox = NativeBashSandbox::new(workspace.path())
        .expect("resolved a3s-sandbox native adapter must construct");
    assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);

    // Do not call the trusted backend probe: user execution must refuse even
    // without a successful process probe, with no alternate runner.
    let legacy_command = format!(
        "printf %s {} > legacy-refusal.txt",
        shell_words::quote(&nonce)
    );
    let legacy_error = sandbox
        .exec_command(&legacy_command, "/workspace")
        .await
        .err()
        .expect("legacy native execution must refuse before spawn");
    assert!(
        legacy_error
            .to_string()
            .contains(NATIVE_LIFETIME_UNQUALIFIED),
        "unexpected legacy refusal: {legacy_error:#}"
    );

    let command = format!(
        "printf %s {} > exec-refusal.txt; printf %s {} > {}",
        shell_words::quote(&nonce),
        shell_words::quote(&nonce),
        shell_words::quote(&outside.to_string_lossy())
    );
    let execution_error = sandbox
        .exec(SandboxCommandRequest {
            command,
            guest_workspace: "/workspace".into(),
            timeout_ms: 15_000,
            output_observer: None,
            env: None,
        })
        .await
        .err()
        .expect("native execution must refuse before spawn");
    assert!(
        execution_error
            .to_string()
            .contains(NATIVE_LIFETIME_UNQUALIFIED),
        "unexpected execution refusal: {execution_error:#}"
    );
    assert!(!workspace.path().join("legacy-refusal.txt").exists());
    assert!(!workspace.path().join("exec-refusal.txt").exists());
    assert!(!outside.exists());
    assert!(!sandbox.supports_background_execution());
    eprintln!(
        "native_lifetime=refused_unqualified; both command APIs refused before sentinel writes; supported_execution=not_qualified; containment=not_qualified; live_support=blocked"
    );
}

/// Existing real fence assertion on platforms whose posture is unchanged.
#[cfg(not(target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn resolved_native_sandbox_denies_outside_workspace_write_on_this_host() {
    let workspace = tempfile::tempdir().expect("workspace");
    let sandbox = NativeBashSandbox::new(workspace.path())
        .expect("resolved a3s-sandbox NativeBashSandbox must initialize on this host");
    assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);
    sandbox
        .probe()
        .await
        .expect("native sandbox probe must succeed before claiming integration");

    let outside =
        std::env::temp_dir().join(format!("a3s-sandbox-escape-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&outside);
    let command = format!(
        "printf 'escaped\\n' > '{}'",
        outside.display().to_string().replace('\'', "'\\''")
    );
    let result = sandbox
        .exec(SandboxCommandRequest {
            command,
            guest_workspace: "/workspace".into(),
            timeout_ms: 15_000,
            output_observer: None,
            env: None,
        })
        .await
        .expect("sandbox exec returns an outcome");

    assert!(
        !outside.exists(),
        "outside-workspace file must not appear on the host; fence failed: exit={} out={} err-path={}",
        result.exit_code,
        result.stdout.chars().take(200).collect::<String>(),
        outside.display()
    );
    // Seatbelt/AppContainer usually surface the deny as a non-zero guest exit.
    // Linux bubblewrap may report exit 0 for a write into an isolated guest
    // `/tmp` that never appears on the host — the host absence above is the
    // authoritative deny signal for this suite.
    if result.exit_code == 0 {
        eprintln!(
            "native fence returned exit 0 with host path absent (isolated guest tmp is acceptable)"
        );
    }
}

/// Live Flash must drive bash through the attached a3s-sandbox native fence.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/deepseek-v4-flash from .a3s/config.acl"]
async fn live_flash_bash_through_a3s_sandbox_writes_workspace_token() {
    // Keep this test registered and failing explicitly on unsupported macOS.
    // This must precede config loading and every potentially billable call.
    assert_live_native_lifetime_supported();
    let workspace = tempfile::tempdir().expect("workspace");
    let sandbox = Arc::new(
        NativeBashSandbox::new(workspace.path()).expect("resolved native a3s-sandbox must init"),
    );
    assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);
    sandbox.probe().await.expect("probe");
    let agent = configured_agent().await;

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options(
                    "native-sandbox-live",
                    &["bash(**)", "write(**)", "read(**)"],
                )
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn BashSandbox>),
            ),
        )
        .await
        .expect("session");

    let target = "native_token.txt";
    let observed = observe(
        &session,
        &format!(
            "Using the bash tool only, write exactly `{NATIVE_TOKEN}` into {target} \
             in the workspace root (printf/redirection is fine). Then stop. Do not invent \
             the token and do not use the write tool."
        ),
    )
    .await;

    let path = workspace.path().join(target);
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    let bash_ok = observed.tools.iter().any(|(name, code, output, meta)| {
        if name != "bash" || *code != 0 {
            return false;
        }
        let sandboxed = meta
            .as_ref()
            .and_then(|value| value.get("sandboxed"))
            .and_then(|value| value.as_bool())
            == Some(true);
        sandboxed && (on_disk.contains(NATIVE_TOKEN) || output.contains(NATIVE_TOKEN))
    });

    assert!(
        bash_ok && on_disk.contains(NATIVE_TOKEN),
        "Flash must create {target} via a3s-sandbox bash (sandboxed=true): disk={on_disk:?} tools={:?} errors={:?} end={}",
        observed
            .tools
            .iter()
            .map(|(n, c, o, m)| (
                n.as_str(),
                *c,
                o.chars().take(120).collect::<String>(),
                m.clone()
            ))
            .collect::<Vec<_>>(),
        observed.errors,
        observed.end_text.chars().take(200).collect::<String>(),
    );
}
