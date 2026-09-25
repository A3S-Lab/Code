//! Live Meta Harness compose E2E (Layer C).
//!
//! Asserts kernel outcomes for a Tardigrade-style `components: [...]` tree that
//! mixes stock parts with a host Moore mount. Oracles are digests, files, and
//! gate terminals — not assistant wording.
//!
//! Opt-in:
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_meta_harness_compose_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::harness_loop::CompletionTerminal;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::verification::{VerificationCheck, VerificationReport, VerificationStatus};
use a3s_code_core::{
    host_component_id, Agent, BuiltinHostHarnessRegistry, CompletionAttestor,
    HarnessComposeOptions, SessionOptions, INTENT_STAMP_MARKER,
};

mod support;
use support::layer_c_model::load_pinned_layer_c_config;

const MODEL_TIMEOUT: Duration = Duration::from_secs(420);
const WRITE_TOKEN: &str = "meta-harness-compose-token-9c2e";

struct DigestBindingAttestor;

impl CompletionAttestor for DigestBindingAttestor {
    fn attest(
        &self,
        digest: &str,
        paths: &[String],
    ) -> Option<a3s_code_core::verification::VerificationReport> {
        if paths.is_empty() {
            return None;
        }
        Some(
            VerificationReport::new(
                "host:meta_harness_live",
                vec![VerificationCheck::required(
                    "host:effect",
                    "host_attestation",
                    "host reconciled composed-harness mutations",
                )
                .with_status(VerificationStatus::Passed)],
            )
            .with_effect_digest(digest.to_string()),
        )
    }
}

async fn configured_agent() -> Agent {
    let config = load_pinned_layer_c_config();
    Agent::from_config(config)
        .await
        .expect("build agent from .a3s/config.acl")
}

fn composed_options(session_id: &str) -> SessionOptions {
    let policy = PermissionPolicy {
        default_decision: PermissionDecision::Deny,
        ..PermissionPolicy::default()
    }
    .allow("read(**)")
    .allow("write(**)")
    .allow("bash(**)");

    let harness = HarnessComposeOptions::compose(
        vec![
            "system".into(),
            "tools".into(),
            host_component_id("intent_stamp"),
            "budget".into(),
            // Intentionally omit compact — subset assemble must still run.
            "infer".into(),
        ],
        Some(6),
        None,
        vec![format!("compose marker {INTENT_STAMP_MARKER}")],
    )
    .expect("compose recipe");

    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_harness(harness)
        .with_host_harness_registry(Arc::new(BuiltinHostHarnessRegistry))
        .with_completion_attestor(Arc::new(DigestBindingAttestor) as Arc<dyn CompletionAttestor>)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn live_composed_harness_write_completes_when_host_attests() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(composed_options("live-meta-harness-compose")),
        )
        .await
        .expect("composed session");

    let prompt = format!(
        "Create compose_out.txt containing exactly the token {WRITE_TOKEN} \
         using the write tool. Then stop. Do not invent verification."
    );
    let result = tokio::time::timeout(MODEL_TIMEOUT, session.send(&prompt, None))
        .await
        .expect("composed harness run timed out");

    let path = workspace.path().join("compose_out.txt");
    assert!(
        path.is_file(),
        "composed harness must still exercise a workspace write; missing {}",
        path.display()
    );
    let contents = std::fs::read_to_string(&path).expect("read compose_out.txt");
    assert!(
        contents.contains(WRITE_TOKEN),
        "write content must carry the host token, got {contents:?}"
    );

    let ok = result.expect("host-attested composed mutation must Allow(Verified)");
    assert!(
        matches!(ok.completion, CompletionTerminal::Verified { .. }),
        "expected Verified on composed path, got {:?}",
        ok.completion
    );
    assert!(
        ok.verification_reports
            .iter()
            .any(|report| report.subject == "host:meta_harness_live"),
        "attestor report must remain visible on the result"
    );
}
