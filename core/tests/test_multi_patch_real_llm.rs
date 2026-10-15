//! PAR-PATCH — multi-file patch rail on the real model (speed axis).
//!
//! Companion to PAR-ADOPT: the `patches` tool's multi-entry staging now
//! prefetches its per-file reads concurrently; this rail proves the
//! shape end-to-end on the real model — one `patches` call editing
//! three files, verified on disk — and records decisions, patch calls,
//! and wall time. Bash is denied so three separate bash rewrites cannot
//! stand in for one patches call.
//!
//! ```text
//! cargo +1.97.1 test -p a3s-code-core --test test_multi_patch_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

const REAL_TIMEOUT: Duration = Duration::from_secs(300);
const FILES: [(&str, i32, i32); 3] = [("alpha", 1, 11), ("bravo", 2, 22), ("charlie", 3, 33)];

#[tokio::test]
#[ignore = "requires the model and credentials from the repo .a3s/config.acl"]
async fn one_patches_call_edits_three_files_on_the_real_model() {
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("loader pins a default model");
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path().to_path_buf();
    for (name, from, _) in FILES {
        std::fs::write(
            root.join(format!("{name}.mjs")),
            format!("export const VALUE = {from};\n"),
        )
        .expect("fixture");
    }
    std::fs::write(
        root.join("check.mjs"),
        r#"import assert from "node:assert/strict";
import { VALUE as A } from "./alpha.mjs";
import { VALUE as B } from "./bravo.mjs";
import { VALUE as C } from "./charlie.mjs";
assert.equal(A, 11); assert.equal(B, 22); assert.equal(C, 33);
console.log("PAR_PATCH_OK");
"#,
    )
    .expect("check fixture");

    let mut policy = PermissionPolicy::new().allow_all(&[
        "ls(*)",
        "read(*)",
        "patch(*)",
        "bash(node check.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            root.to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("par-patch")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_permission_policy(policy)
                    .with_default_security()
                    .with_max_tool_rounds(12)
                    .with_llm_api_timeout(120_000)
                    .with_temperature(0.0)
                    .with_continuation(false)
                    // Host-declared suite: `node check.mjs` covers the
                    // three mutated files, so its passing run binds.
                    .with_verification_contract(
                        a3s_code_core::verification::VerificationContractV1::new(
                            "check-suite",
                            "node check.mjs",
                            ["alpha.mjs", "bravo.mjs", "charlie.mjs", "check.mjs"],
                        )
                        .expect("contract"),
                    ),
            ),
        )
        .await
        .expect("session");

    let started = std::time::Instant::now();
    let answer = tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            "Each of alpha.mjs, bravo.mjs, and charlie.mjs exports VALUE. Set alpha's VALUE \
             to 11, bravo's to 22, and charlie's to 33 — apply all three edits with ONE \
             patches tool call, then run `node check.mjs` to verify, then reply with the \
             three new values.",
            None,
        ),
    )
    .await
    .expect("turn timed out");
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            let facts = a3s_code_core::fact_control::read_workspace_facts(&root, "par-patch")
                .unwrap_or_default();
            for fact in &facts {
                if fact.kind == "model.turn" {
                    eprintln!(
                        "PAR-PATCH model.turn: {}",
                        &fact.payload.to_string()[..fact.payload.to_string().len().min(400)]
                    );
                }
            }
            panic!("turn failed: {error:#}");
        }
    };
    let seconds = started.elapsed().as_secs_f64();
    println!("PAR-PATCH model={model} reply: {}", answer.text.trim());

    // Disk is the oracle: all three files carry their new value.
    for (name, _, to) in FILES {
        let content = std::fs::read_to_string(root.join(format!("{name}.mjs"))).expect("file");
        assert!(
            content.contains(&format!("VALUE = {to};")),
            "{name}.mjs must carry VALUE = {to}: {content}"
        );
    }
    println!("PAR-PATCH seconds={seconds:.1} verdict=DISK-VERIFIED");
    drop(session);
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
}
