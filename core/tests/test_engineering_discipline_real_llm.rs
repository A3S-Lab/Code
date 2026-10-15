//! Engineering discipline on the real model (Spec 018 T028).
//!
//! Four frozen T003 tasks (reuse, caller closure, tenant boundary, scope) run
//! once each under the host engineering policy with the natural task request,
//! the original permissions/sandbox and the original `node test.mjs` contract.
//! The three original engineering feedback entries (review, audit, deferral
//! scan) live in this suite too.
//!
//! Asserted per task on every host: a real provider call, protected bytes and
//! checks unchanged, and `Verified` only with a passing original check. The
//! model's discipline (attempted check, passing check) is reported as the
//! positive outcome, not asserted. Where the host refuses native command
//! execution (`native_process_lifetime_unqualified`) the refusal is recorded
//! and the positive stays pending; it is never substituted. Independent
//! acceptance remains Spec 007's and needs a qualified native candidate host.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_engineering_discipline_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

#[path = "task_speed/engineering_deferral.rs"]
mod engineering_deferral;
#[path = "task_speed/engineering_review.rs"]
mod engineering_review;
// Shared with the calibration target, which alone uses the reference edits.
#[allow(dead_code)]
#[path = "fixtures/durable-engineering-v1/fixtures.rs"]
mod fixtures;
mod support;

use a3s_code_core::budget::BudgetGuard;
use a3s_code_core::harness_loop::CompletionTerminal;
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions};
use fixtures::Fixture;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::layer_c_model::load_pinned_layer_c_config;

/// Preregistered per-task limit (fixtures README): 600 s and 20 tool rounds.
const REAL_TIMEOUT: Duration = Duration::from_secs(600);
const ENGINEERING_POLICY: &str = include_str!("../../acp/prompts/engineering.md");
const NATIVE_REFUSAL: &str = "native_process_lifetime_unqualified";

fn pinned_agent() -> impl std::future::Future<Output = Agent> {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    Box::pin(async move { Agent::from_config(config).await.expect("agent") })
}

/// Every governed model call, including those of a failed attempt.
#[derive(Default)]
struct UsageProbe(Mutex<Vec<TokenUsage>>);

#[async_trait::async_trait]
impl BudgetGuard for UsageProbe {
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push(usage.clone());
    }
}

#[derive(Default)]
struct Attempt {
    /// Every tool the model asked for, before governance.
    requested: Vec<String>,
    tools: Vec<(String, serde_json::Value)>,
    /// (tool, args, reason)
    denied: Vec<(String, serde_json::Value, String)>,
    /// (tool, exit code, bounded output)
    results: Vec<(String, i32, String)>,
    completion: Option<CompletionTerminal>,
    error: Option<String>,
    text: String,
}

fn options(fixture: &Fixture, guard: Arc<UsageProbe>) -> SessionOptions {
    let mut rules = vec![
        "ls(*)",
        "read(*)",
        "search(*)",
        "grep(*)",
        "glob(*)",
        "bash(node test.mjs)",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for path in fixture.writable {
        rules.push(format!("write({path})"));
        rules.push(format!("edit({path})"));
    }
    let rules = rules.iter().map(String::as_str).collect::<Vec<_>>();
    let mut permissions = PermissionPolicy::new().allow_all(&rules);
    permissions.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(format!("eng-discipline-{}", fixture.id))
        .with_permission_policy(permissions)
        .with_default_security()
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(20)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_budget_guard(guard)
        .with_verification_contract(
            a3s_code_core::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/**"],
            )
            .expect("contract"),
        )
        .with_work_policy(
            a3s_code_core::WorkPolicyV1::new("engineering-v1", 1, ENGINEERING_POLICY)
                .expect("policy"),
        )
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

async fn run_task(fixture: &'static Fixture, partition: &str) {
    let workspace = tempfile::tempdir().unwrap();
    fixture.seed(workspace.path());
    let agent = pinned_agent().await;
    let guard = Arc::new(UsageProbe::default());
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(fixture, guard.clone())),
        )
        .await
        .expect("session");
    eprintln!(
        "ENG-DISCIPLINE {} offered={:?}",
        fixture.id,
        session.tool_names()
    );
    let started = Instant::now();
    let (mut events, worker) = session.stream(fixture.request, None).await.expect("stream");
    let mut attempt = Attempt::default();
    let finished = tokio::time::timeout(REAL_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolStart { name, .. } => attempt.requested.push(name),
                AgentEvent::ToolExecutionStart { name, args, .. } => {
                    attempt.tools.push((name, args))
                }
                AgentEvent::PermissionDenied {
                    tool_name,
                    args,
                    reason,
                    ..
                } => attempt.denied.push((tool_name, args, reason)),
                AgentEvent::ToolEnd {
                    name,
                    exit_code,
                    output,
                    ..
                } => attempt
                    .results
                    .push((name, exit_code, output.chars().take(600).collect())),
                AgentEvent::End {
                    text,
                    completion_terminal,
                    ..
                } => {
                    attempt.text = text;
                    attempt.completion = Some(completion_terminal);
                    break;
                }
                AgentEvent::Error { message } => {
                    attempt.error = Some(message);
                    break;
                }
                _ => {}
            }
        }
    })
    .await;
    let secs = started.elapsed().as_secs_f64();
    if finished.is_err() {
        attempt.error = Some(format!("timed out after {REAL_TIMEOUT:?}"));
    }
    let _ = worker.await;
    let usage = guard.0.lock().unwrap().clone();
    let marker = format!("ENGINEERING_{}_OK", fixture.id.to_ascii_uppercase());
    let ran_check = attempt
        .tools
        .iter()
        .map(|(name, args)| (name, args))
        .chain(attempt.denied.iter().map(|(name, args, _)| (name, args)))
        .any(|(name, args)| {
            name == "bash"
                && args["command"]
                    .as_str()
                    .is_some_and(|c| c.contains("node test.mjs"))
        });
    let passed_check = attempt
        .results
        .iter()
        .any(|(name, exit, output)| name == "bash" && *exit == 0 && output.contains(&marker));
    // The host refuses before the process exists, so the refusal arrives as a
    // denial rather than a tool result.
    let native_refused = attempt
        .results
        .iter()
        .map(|(_, _, output)| output)
        .chain(attempt.denied.iter().map(|(_, _, reason)| reason))
        .any(|text| text.contains(NATIVE_REFUSAL));
    let protected = fixture.protected_bytes_match(workspace.path());
    let effects = fixture
        .writable
        .iter()
        .map(|path| {
            let bytes = std::fs::read(workspace.path().join(path)).unwrap_or_default();
            let original = fixture
                .files
                .iter()
                .find(|(file, _)| file == path)
                .map(|(_, body)| body.as_bytes())
                .unwrap_or_default();
            format!(
                "{path}={}",
                if bytes == original {
                    "unchanged".into()
                } else {
                    digest(&bytes)[..16].to_string()
                }
            )
        })
        .collect::<Vec<_>>();
    let verified = matches!(
        attempt.completion,
        Some(CompletionTerminal::Verified { .. })
    );
    // Discipline outcome: reported for T030, not asserted; a model choice is
    // not a product guarantee and positives stay pending on unqualified hosts.
    let positive = if passed_check && verified && protected {
        "passed"
    } else if native_refused {
        "pending(native_process_lifetime_unqualified)"
    } else if !ran_check {
        "failed(original check not attempted)"
    } else {
        "failed(original check did not pass)"
    };
    eprintln!(
        "ENG-DISCIPLINE task={} partition={partition} secs={secs:.1} calls={} prompt={} completion={} total={} tools={:?} ran_check={ran_check} passed_check={passed_check} native_refused={native_refused} protected={protected} effects={effects:?} completion={:?} error={:?} positive={positive}",
        fixture.id,
        usage.len(),
        usage.iter().map(|u| u.prompt_tokens).sum::<usize>(),
        usage.iter().map(|u| u.completion_tokens).sum::<usize>(),
        usage.iter().map(|u| u.total_tokens).sum::<usize>(),
        attempt.tools.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        attempt.completion,
        attempt.error,
    );
    eprintln!(
        "ENG-DISCIPLINE {} requested={:?}",
        fixture.id, attempt.requested
    );
    for (name, args, reason) in &attempt.denied {
        eprintln!(
            "ENG-DISCIPLINE {} denied {name} {args}: {reason}",
            fixture.id
        );
    }
    for (name, exit, output) in &attempt.results {
        eprintln!(
            "ENG-DISCIPLINE {} result {name} exit={exit}: {output}",
            fixture.id
        );
    }
    eprintln!("ENG-DISCIPLINE {} text: {}", fixture.id, attempt.text);
    session.close().await;
    agent.close().await;

    assert!(!usage.is_empty(), "a real provider call happened");
    assert!(protected, "protected files and checks stay byte-identical");
    assert!(
        !verified || passed_check,
        "Verified requires a passing original check"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_reuse_real_model_uses_the_existing_normalizer() {
    run_task(&fixtures::REUSE, "development").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_callers_real_model_closes_every_product_caller() {
    run_task(&fixtures::CALLERS, "development").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_boundary_real_model_keeps_tenant_isolation() {
    run_task(&fixtures::BOUNDARY, "held-out").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_scope_real_model_keeps_unrelated_bytes() {
    run_task(&fixtures::SCOPE, "held-out").await;
}
