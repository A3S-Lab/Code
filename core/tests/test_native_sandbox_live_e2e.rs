//! Live E2E: a3s-code drives bash through **a3s-sandbox 0.1.3** (native OS fence).
//!
//! First principles:
//! - Integration means the default local session path uses `NativeBashSandbox`
//!   (Seatbelt / namespaces / AppContainer), not process-host fallback.
//! - Passes require kernel effects: probe, sandboxed tool metadata, on-disk
//!   side effects, and a real outside-workspace deny. Assistant wording is
//!   never a pass criterion.
//! - Model pin comes from monorepo `.a3s/config.acl` Layer C Flash
//!   (`boyue/bailian/deepseek-v4.1-flash` — the bailian Flash route; there is
//!   no `boyue/bailian/deepseek-v4-flash` id in that file).
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/to/a3s/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_native_sandbox_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::sandbox::native::{NativeBashSandbox, NATIVE_SANDBOX_BACKEND};
use a3s_code_core::sandbox::{BashSandbox, SandboxCommandRequest};
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use support::layer_c_model::{load_pinned_layer_c_config, REQUIRED_DEFAULT_MODEL};

const MODEL_TIMEOUT: Duration = Duration::from_secs(240);
const NATIVE_TOKEN: &str = "native-sandbox-live-token-0a3s";

async fn configured_agent() -> Agent {
    let config = load_pinned_layer_c_config();
    assert_eq!(
        config.default_model.as_deref(),
        Some(REQUIRED_DEFAULT_MODEL),
        "live native-sandbox suite must pin {REQUIRED_DEFAULT_MODEL}"
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
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(90_000)
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

/// Direct fence check: a3s-sandbox must refuse writing outside the workspace.
#[tokio::test(flavor = "multi_thread")]
async fn a3s_sandbox_0_1_3_denies_outside_workspace_write_on_this_host() {
    let workspace = tempfile::tempdir().expect("workspace");
    let sandbox = NativeBashSandbox::new(workspace.path()).expect(
        "a3s-sandbox 0.1.3 NativeBashSandbox must initialize on this host (Seatbelt/bwrap/AppContainer)",
    );
    assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);
    sandbox
        .probe()
        .await
        .expect("native sandbox probe must succeed before claiming integration");

    let outside = PathBuf::from(std::env::temp_dir())
        .join(format!("a3s-sandbox-escape-{}.txt", std::process::id()));
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
        "outside-workspace file must not appear; fence failed: exit={} out={} err-path={}",
        result.exit_code,
        result.stdout.chars().take(200).collect::<String>(),
        outside.display()
    );
    assert!(
        result.exit_code != 0,
        "outside write must be a non-zero guest exit under native fence; got 0 with stdout={}",
        result.stdout.chars().take(200).collect::<String>()
    );
}

/// Live Flash must drive bash through the attached a3s-sandbox native fence.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires boyue/bailian/deepseek-v4.1-flash from .a3s/config.acl"]
async fn live_flash_bash_through_a3s_sandbox_writes_workspace_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let sandbox = Arc::new(
        NativeBashSandbox::new(workspace.path()).expect("native a3s-sandbox 0.1.3 must init"),
    );
    assert_eq!(sandbox.backend(), NATIVE_SANDBOX_BACKEND);
    sandbox.probe().await.expect("probe");

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
