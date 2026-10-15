//! Long-horizon capability matrix on the real model — planning, parallel
//! batching, and fault recovery inside ONE bounded workflow (the recovery
//! rail's interrupt/checkpoint axis keeps its own suites:
//! `test_fault_recovery_real_llm`, `test_exact_id_checkpoint_real_llm`).
//!
//! Mechanical oracles (no prose judgment):
//! - PLAN: a `model.turn` fact carries an `update_plan` call;
//! - PAR: a `model.turn` fact carries ≥2 tool calls in one decision
//!   (the batch shape; the overlap guarantee itself is pinned hermetically
//!   by the WC-MEAS2 barrier oracle);
//! - FAULT→RECOVER: the first verification run fails on record, the model
//!   still lands the fix, and the passing rerun prints the success sentinel
//!   — a string that exists nowhere in the prompt's data until the fixed
//!   test emits it — into a `tool.result` fact;
//! - COMPLETE: the run settles `Verified` through the host-declared suite
//!   contract (`VerificationContractV1`): the session manifest admits
//!   `node test.mjs` as the project's suite and covers the mutated path,
//!   so a bare suite run binds instead of being refused (the gap the
//!   `agent_api/tests/long_horizon_capabilities.rs` twins pin hermetically).
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_long_horizon_capabilities_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::store::SessionStore;
use a3s_code_core::{Agent, PlanningMode, SessionOptions, SystemPromptSlots};
use support::layer_c_model::load_pinned_layer_c_config;

const REAL_TIMEOUT: Duration = Duration::from_secs(600);
const SUCCESS_SENTINEL: &str = "LH4_EXPECTED_SHIP_9d2c";
const FAIL_SENTINEL: &str = "LH4_WRONG_SUM";

const SPEC: &str = "# Ship-cost contract\n\n`shipCost(base, a, b, c)` returns `base + a + b + c` where a, b, c come from the three data files. The buggy implementation multiplies instead of adding and misses one file.\n";

const TEST_SOURCE: &str = r#"import assert from "node:assert/strict";
import { shipCost } from "./src/impl.mjs";
import { A } from "./data/a.mjs";
import { B } from "./data/b.mjs";
import { C } from "./data/c.mjs";

const actual = shipCost(10, A, B, C);
assert.deepEqual(actual, 10 + A + B + C, "LH4_WRONG_SUM");
console.log("LH4_EXPECTED_SHIP_9d2c");
"#;

fn data(name: &str, value: i32) -> String {
    format!("export const {name} = {value};\n")
}

const BROKEN_IMPL: &str = r#"import { A } from "../data/a.mjs";
import { B } from "../data/b.mjs";

export function shipCost(base, a, b, c) {
    return base * a * b;
}
"#;

fn seed_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("SPEC.md"), SPEC).unwrap();
    std::fs::write(root.join("test.mjs"), TEST_SOURCE).unwrap();
    std::fs::write(root.join("data/a.mjs"), data("A", 2)).unwrap();
    std::fs::write(root.join("data/b.mjs"), data("B", 3)).unwrap();
    std::fs::write(root.join("data/c.mjs"), data("C", 5)).unwrap();
    std::fs::write(root.join("src/impl.mjs"), BROKEN_IMPL).unwrap();
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn plan_parallel_and_fault_recovery_complete_one_long_task() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path().to_path_buf();
    seed_workspace(&root);

    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");
    let mut policy = PermissionPolicy::new().allow_all(&[
        "ls(*)",
        "read(*)",
        "update_plan(*)",
        "write(src/**)",
        "edit(src/**)",
        "bash(node test.mjs)",
        "bash(test -f src/impl.mjs && node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            root.to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("lh4-capabilities")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_permission_policy(policy)
                    .with_default_security()
                    .with_planning_mode(PlanningMode::Enabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_max_tool_rounds(20)
                    .with_llm_api_timeout(180_000)
                    .with_temperature(0.0)
                    .with_continuation(false)
                    // Host manifest: this workspace's suite is `node test.mjs`
                    // and it exercises src/**. A passing run of it verifies
                    // the mutated paths — the model cannot grant this.
                    .with_verification_contract(
                        a3s_code_core::verification::VerificationContractV1::new(
                            "node-suite",
                            "node test.mjs",
                            ["src/**"],
                        )
                        .expect("contract"),
                    )
                    .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
                        "This is a coding conformance task with a required tool choreography: call update_plan first, read the three data files in ONE assistant turn as three parallel read calls, then verify-fail, fix only src/impl.mjs, then verify again. Apply every change with real tool calls — never reply with a proposed or predicted patch, and do not re-read files you have already read.",
                    )),
            ),
        )
        .await
        .expect("session");

    let answer = tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            "Repair this project with this exact choreography:\n\
             1. Call update_plan with 3 steps: inspect, fix, verify.\n\
             2. Read data/a.mjs, data/b.mjs, data/c.mjs in ONE assistant turn using three parallel read calls.\n\
             3. Run exactly `node test.mjs` and observe the failure — keep going.\n\
             4. Fix src/impl.mjs per SPEC.md (add, do not multiply; include all three values).\n\
             5. Run exactly `node test.mjs` again to prove the repair.",
            None,
        ),
    )
    .await
    .expect("turn timed out")
    .expect("turn failed");
    println!("LH4 reply: {}", answer.text.trim());

    let facts = a3s_code_core::fact_control::read_workspace_facts(&root, "lh4-capabilities")
        .expect("facts");

    // PLAN oracle: an update_plan call is on record.
    let plan_call = facts
        .iter()
        .filter(|fact| fact.kind == "model.turn")
        .any(|fact| {
            fact.payload
                .to_string()
                .contains("\"name\":\"update_plan\"")
                || fact
                    .payload
                    .to_string()
                    .contains("\"name\": \"update_plan\"")
        });
    assert!(plan_call, "an update_plan call must be on the fact log");

    // PAR oracle: one model decision carried ≥2 tool calls.
    let batch = facts
        .iter()
        .filter(|fact| fact.kind == "model.turn")
        .any(|fact| {
            let rendered = fact.payload.to_string();
            rendered.contains("\"also\"")
                && rendered.contains("\"call\"")
                && rendered.matches("\"id\"").count() >= 2
        });
    assert!(
        batch,
        "at least one model turn must carry a multi-call batch"
    );

    // FAULT→RECOVER oracle: the failure is on record, the fix landed on
    // disk, and the passing rerun printed the success sentinel — a string
    // that exists nowhere in the prompt's data until the fixed test emits
    // it — into a tool.result fact.
    let failure_recorded = facts.iter().any(|fact| {
        fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(FAIL_SENTINEL))
    });
    assert!(
        failure_recorded,
        "the first failing verification must be on the fact log"
    );
    let fixed = std::fs::read_to_string(root.join("src/impl.mjs")).expect("impl survives the turn");
    let _ = FAIL_SENTINEL;
    assert!(fixed.contains("+"), "the fix must replace multiplication");
    let success_recorded = facts.iter().any(|fact| {
        fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(SUCCESS_SENTINEL))
    });
    assert!(
        success_recorded,
        "the passing rerun must print the success sentinel onto the fact log"
    );
    // COMPLETE oracle: the declared-suite contract binds the passing run,
    // so the run settles Verified — not the refusal the earlier live runs
    // died on, and not a narrative allow either.
    assert!(
        matches!(
            answer.completion,
            a3s_code_core::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "the declared suite contract must complete the run Verified; got {:?} (answer: {})",
        answer.completion,
        answer.text.trim()
    );
    assert!(
        answer.verification_reports.iter().any(|report| {
            report.subject == "shell:contract:node-suite" && report.effect_digest.is_some()
        }),
        "the bound contract report must be on the result"
    );
    println!("LH4 verdict: plan=recorded batch=recorded fault=recovered suite=verified");
}

// --- Live recovery axis: real crash mid-task → checkpoint resume → Verified
//
// The plan/parallel/fault leg above runs one uninterrupted workflow. This
// axis kills the PROCESS mid-task — the only true crash — and resumes the
// run in a FRESH process from the per-tool-round checkpoint. The mutation
// ledger rides the checkpoint, so the resumed completion gate still sees
// the whole task's effect and the declared-suite contract binds the FULL
// effect, not just the post-resume fragment.
//
// Both legs are helpers spawned from the parent test through the test
// binary itself (the cross-process shape the recovery contract is for).

const LH4_RECOVERY_ROLE_ENV: &str = "LH4_RECOVERY_ROLE";
const LH4_RECOVERY_WORKSPACE_ENV: &str = "LH4_RECOVERY_WORKSPACE";
const LH4_RECOVERY_STORE_ENV: &str = "LH4_RECOVERY_STORE";
const LH4_SOURCE_RUN: &str = "lh4-live-src";
const LH4_RESUMED_RUN: &str = "lh4-live-dst";
const CRASH_EXIT_CODE: i32 = 70;

fn recovery_session_options(store_dir: &Path) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&[
        "ls(*)",
        "read(*)",
        "write(src/**)",
        "edit(src/**)",
        "bash(node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id("lh4-live-recovery")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(12)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_file_session_store(store_dir.to_path_buf())
        .with_verification_contract(
            a3s_code_core::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/**"],
            )
            .expect("contract"),
        )
}

async fn recovery_agent() -> Agent {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    Agent::from_config(config).await.expect("agent")
}

fn wait_fixed(root: &Path) -> bool {
    std::fs::read_to_string(root.join("src/impl.mjs"))
        .map(|content| content.contains("base + a + b + c") || content.contains('+'))
        .unwrap_or(false)
}

/// Crash leg: run the repair on the real model; once the fix AND the
/// checkpoint with the durable ledger are on disk, die without settling.
#[tokio::test(flavor = "multi_thread")]
async fn lh4_recovery_helper() {
    let Ok(role) = std::env::var(LH4_RECOVERY_ROLE_ENV) else {
        return; // spawned only by the parent below
    };
    let root = PathBuf::from(std::env::var(LH4_RECOVERY_WORKSPACE_ENV).expect("workspace"));
    let store_dir = PathBuf::from(std::env::var(LH4_RECOVERY_STORE_ENV).expect("store"));

    if role == "crash" {
        let agent = recovery_agent().await;
        let session = agent
            .session_async(
                root.to_string_lossy().to_string(),
                Some(recovery_session_options(&store_dir)),
            )
            .await
            .expect("crash session");
        let spawn = session
            .spawn_run_with_id(
                LH4_SOURCE_RUN,
                "Fix src/impl.mjs according to SPEC.md (add, do not multiply; use all \
                 three values a, b, c). Apply the fix with the edit tool — never reply \
                 with a proposed patch. Then run exactly `node test.mjs` to verify. \
                 Then stop.",
            )
            .await
            .expect("spawn crash run");
        assert!(!spawn.replayed(), "fresh run id must not replay");

        // The durable boundary: the fix landed AND the committed checkpoint
        // carries the mutation ledger. Then die — no publish, no cleanup.
        let poll_store = a3s_code_core::store::FileSessionStore::new(&store_dir)
            .await
            .expect("open store");
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "the fix/checkpoint never committed before the deadline"
            );
            let checkpoint_committed = poll_store
                .load_loop_checkpoint(LH4_SOURCE_RUN)
                .await
                .expect("checkpoint load")
                .is_some_and(|checkpoint| {
                    checkpoint.convergence.mutations
                        != a3s_code_core::harness_loop::MutationLedger::default()
                });
            if wait_fixed(&root) && checkpoint_committed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        std::process::exit(CRASH_EXIT_CODE);
    }

    if role == "recover" {
        let agent = recovery_agent().await;
        let session = agent
            .session_async(
                root.to_string_lossy().to_string(),
                Some(recovery_session_options(&store_dir)),
            )
            .await
            .expect("recover session");
        let spawn = session
            .spawn_recovery_with_run_id(LH4_SOURCE_RUN, LH4_RESUMED_RUN)
            .await
            .expect("spawn recovery run");
        let worker = match spawn {
            a3s_code_core::AgentRunSpawn::Started { worker, .. } => worker,
            a3s_code_core::AgentRunSpawn::Replayed { .. } => {
                panic!("fresh recovery id must not replay")
            }
        };

        let status = tokio::time::timeout(Duration::from_secs(420), async {
            loop {
                if let Some(snapshot) = session.run_snapshot(LH4_RESUMED_RUN).await {
                    if snapshot.status.is_terminal() {
                        return snapshot.status;
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("resumed run never settled");
        assert_eq!(
            status,
            a3s_code_core::run::RunStatus::Completed,
            "the resumed long task must complete; events: {:?}",
            session.run_events(LH4_RESUMED_RUN).await
        );
        let end = session
            .run_events(LH4_RESUMED_RUN)
            .await
            .into_iter()
            .find_map(|record| match record.event {
                a3s_code_core::AgentEvent::End { text, .. } => Some(text),
                _ => None,
            });
        assert_eq!(
            end.as_deref(),
            Some("completed"),
            "the resumed run must settle through the verified completion"
        );
        // The fact thread is keyed by the session id, identical across the
        // crash and recover processes; a read of an unknown thread returns
        // Ok(empty), so there is no fallback chain here — read the one
        // thread directly.
        let facts = a3s_code_core::fact_control::read_workspace_facts(
            &root,
            &a3s_code_core::fact_control::thread_for_session("lh4-live-recovery"),
        )
        .expect("facts");
        assert!(
            facts.iter().any(|fact| fact.kind == "tool.result"
                && fact
                    .payload
                    .get("output")
                    .and_then(|value| value.as_str())
                    .is_some_and(|output| output.contains(SUCCESS_SENTINEL))),
            "the resumed suite run must print the success sentinel onto the fact log"
        );
        assert!(wait_fixed(&root), "the pre-crash fix must still be on disk");
        // Preserve the detached observer policy; terminal RunStore state is
        // not a proof that this worker has joined.
        drop(worker);
        agent.close().await;
        println!("LH4 recovery verdict: crash=survived resume=completed suite=verified");
        return;
    }

    panic!("unknown recovery role: {role}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn crashed_long_task_resumes_across_processes_and_completes_verified() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let root = workspace.path().to_path_buf();
    seed_workspace(&root);

    let executable = std::env::current_exe().expect("test binary");

    async fn run_helper(
        executable: PathBuf,
        role: &str,
        root: &Path,
        store: &Path,
    ) -> std::process::Output {
        tokio::process::Command::new(executable)
            .args([
                "--exact",
                "lh4_recovery_helper",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LH4_RECOVERY_ROLE_ENV, role)
            .env(LH4_RECOVERY_WORKSPACE_ENV, root)
            .env(LH4_RECOVERY_STORE_ENV, store)
            .kill_on_drop(true)
            .output()
            .await
            .expect("spawn helper")
    }

    let crash = tokio::time::timeout(
        Duration::from_secs(420),
        run_helper(executable.clone(), "crash", &root, store_dir.path()),
    )
    .await
    .expect("crash leg timed out");
    assert_eq!(
        crash.status.code(),
        Some(CRASH_EXIT_CODE),
        "the crash leg must die at the durable boundary; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&crash.stdout),
        String::from_utf8_lossy(&crash.stderr),
    );

    let recover = tokio::time::timeout(
        Duration::from_secs(480),
        run_helper(executable, "recover", &root, store_dir.path()),
    )
    .await
    .expect("recover leg timed out");
    assert!(
        recover.status.success(),
        "the recovery leg must complete verified; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr),
    );
    println!(
        "LH4 cross-process recovery: crash at durable boundary, resumed run completed verified"
    );
}

// --- Live continuation axis: graceful interrupt → next send → Verified -----
//
// The crash axis resumes through a checkpoint in a fresh process. A
// GRACEFUL interrupt is the everyday long-horizon seam: the run settles
// Cancelled (and clears its checkpoint), and continuation is the NEXT SEND
// on the same session. That send's gate must inherit the interrupted run's
// mutations — replayed from the fact log after the last `turn.complete`
// marker — or it would drift to Narrative over an effect it never
// accounted for.

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn interrupted_long_task_continues_into_a_verified_completion() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store_dir = tempfile::tempdir().expect("store");
    let root = workspace.path().to_path_buf();
    seed_workspace(&root);

    let agent = recovery_agent().await;
    let session = agent
        .session_async(
            root.to_string_lossy().to_string(),
            Some(
                recovery_session_options(store_dir.path()).with_session_id("lh4-live-continuation"),
            ),
        )
        .await
        .expect("continuation session");

    let (mut events, worker) = session
        .stream(
            "Fix src/impl.mjs according to SPEC.md (add, do not multiply; use all \
             three values a, b, c). Apply the fix with the edit tool — never reply \
             with a proposed patch. Then run exactly `node test.mjs` to verify. \
             Then stop.",
            None,
        )
        .await
        .expect("start interrupted stream");
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    // Interrupt mid-task: the fix on disk means the mutation landed; the
    // model is between rounds. If the run completed first, the interrupt
    // lost the race — fail loudly for the bounded retry.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the fix never landed before the deadline"
        );
        // The fact, not the file, is the accountability boundary: if the
        // interrupt lands between the edit's disk write and its
        // `tool.result` fact, the mutation is real but invisible to the
        // replay. Wait for the fact to be on the log.
        let fact_committed = a3s_code_core::fact_control::read_workspace_facts(
            &root,
            &a3s_code_core::fact_control::thread_for_session("lh4-live-continuation"),
        )
        .map(|facts| facts.iter().any(|fact| fact.kind == "tool.result"))
        .unwrap_or(false);
        if wait_fixed(&root) && fact_committed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let snapshot = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(snapshot) = session.run_control_snapshot().await {
                if snapshot.turn_id.is_some() {
                    return snapshot;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no active run-control snapshot mid-task");
    let receipt = session
        .interrupt(
            a3s_code_core::run_control::InterruptRequest::new()
                .with_reason("continuation leg")
                .with_run_id(snapshot.run_id.clone()),
        )
        .await
        .expect("interrupt rejected");
    assert_eq!(
        receipt.state,
        a3s_code_core::run_control::RunControlReceiptState::Accepted
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while session.run_control_snapshot().await.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("interrupted stream never settled");

    // The continuation send: the gate inherits the interrupted run's edit,
    // so the passing suite binds the FULL effect and the run settles
    // through the verified short-circuit.
    let answer = tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            "The fix from the interrupted run is already applied. Run exactly \
             `node test.mjs` to verify it, then stop.",
            None,
        ),
    )
    .await
    .expect("continuation send timed out")
    .expect("continuation send failed");
    assert!(
        matches!(
            answer.completion,
            a3s_code_core::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "the continuation must complete Verified over the inherited effect; got {:?}",
        answer.completion
    );
    assert_eq!(answer.text, "completed");

    let facts = a3s_code_core::fact_control::read_workspace_facts(
        &root,
        &a3s_code_core::fact_control::thread_for_session("lh4-live-continuation"),
    )
    .expect("facts");
    assert!(
        facts.iter().any(|fact| fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(SUCCESS_SENTINEL))),
        "the continuation's suite run must print the success sentinel"
    );
    assert!(
        facts
            .iter()
            .any(|fact| fact.kind == a3s_code_core::fact_control::TURN_COMPLETE_FACT_KIND),
        "the settled continuation must append the turn.complete marker"
    );
    drop(worker);
    println!("LH4 continuation verdict: interrupt=settled continuation=verified");
    session.close().await;
    agent.close().await;
}
