//! Real-LLM end-to-end tests for the cluster-grade features added in 3.3.0
//! (BudgetGuard enforcement, loop-checkpoint lifecycle, resume_run, identity
//! labels). These exercise code paths that mock LLM clients cannot validate —
//! most importantly that `BudgetGuard::record_after_llm` receives the
//! provider's *actual* token usage and that a real run's lifecycle clears its
//! checkpoint.
//!
//! All `#[ignore]` — they require a live provider in `.a3s/config.acl`. Run:
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_real_llm_cluster_features -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_code_core::budget::{BudgetDecision, BudgetGuard};
use a3s_code_core::config::CodeConfig;
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{Agent, SessionOptions};

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn real_agent() -> Agent {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to load {}: {e}", path.display()));
    Agent::from_config(config)
        .await
        .expect("agent from real config")
}

// A guard that always denies, counting how many times it was consulted.
#[derive(Default)]
struct DenyGuard {
    checks: AtomicUsize,
    records: AtomicUsize,
}

#[async_trait::async_trait]
impl BudgetGuard for DenyGuard {
    async fn check_before_llm(&self, _session_id: &str, _est: usize) -> BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        BudgetDecision::Deny {
            resource: "llm_tokens".to_string(),
            reason: "test cap exceeded".to_string(),
        }
    }
    async fn record_after_llm(&self, _session_id: &str, _usage: &TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
    }
}

// A guard that allows but captures the *actual* usage the provider reports.
#[derive(Default)]
struct RecordingGuard {
    checks: AtomicUsize,
    records: AtomicUsize,
    last_total_tokens: AtomicU64,
}

#[async_trait::async_trait]
impl BudgetGuard for RecordingGuard {
    async fn check_before_llm(&self, _session_id: &str, _est: usize) -> BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _session_id: &str, usage: &TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
        self.last_total_tokens
            .store(usage.total_tokens as u64, Ordering::SeqCst);
    }
}

/// A real `Deny` from `check_before_llm` must abort the call BEFORE the
/// provider is contacted: send errors with "Budget exhausted", the guard
/// was consulted exactly once, `record_after_llm` never fired, and no
/// conversation history was recorded.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_budget_guard_deny_blocks_llm_call() {
    let guard = Arc::new(DenyGuard::default());
    let agent = real_agent().await;
    let opts = SessionOptions::new()
        .with_session_id("real-budget-deny")
        .with_budget_guard(guard.clone() as Arc<dyn BudgetGuard>);
    let session = agent
        .session_async("/tmp/real-budget-deny", Some(opts))
        .await
        .expect("session");

    let err = session
        .send("Reply with the single word: ok", None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Budget exhausted"),
        "expected budget-exhausted error, got: {err}"
    );
    assert_eq!(
        guard.checks.load(Ordering::SeqCst),
        1,
        "guard consulted once"
    );
    assert_eq!(
        guard.records.load(Ordering::SeqCst),
        0,
        "record_after_llm must not fire when denied (LLM never called)"
    );
    assert!(
        session.history().is_empty(),
        "denied call must not record history"
    );
}

/// On `Allow`, the real run completes and `record_after_llm` receives the
/// provider's ACTUAL non-zero token usage — the post-call accounting path a
/// mock client (which returns fixed/zero usage) cannot validate.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_budget_guard_allow_records_actual_usage() {
    let guard = Arc::new(RecordingGuard::default());
    let agent = real_agent().await;
    let opts = SessionOptions::new()
        .with_session_id("real-budget-allow")
        .with_budget_guard(guard.clone() as Arc<dyn BudgetGuard>);
    let session = agent
        .session_async("/tmp/real-budget-allow", Some(opts))
        .await
        .expect("session");

    let result = session
        .send("Reply with the single word: ok", None)
        .await
        .expect("real send should succeed under an allowing guard");

    assert!(!result.text.is_empty(), "real model returned text");
    assert!(guard.checks.load(Ordering::SeqCst) >= 1, "guard consulted");
    assert!(
        guard.records.load(Ordering::SeqCst) >= 1,
        "record_after_llm must fire on a successful real call"
    );
    assert!(
        guard.last_total_tokens.load(Ordering::SeqCst) > 0,
        "record_after_llm must receive the provider's real (non-zero) token usage"
    );
    assert!(
        result.usage.total_tokens > 0,
        "AgentResult must carry real token usage"
    );
}

/// A real run with a `SessionStore` configured must, on completion, leave NO
/// dangling loop checkpoint for its run id — the leak-fix lifecycle path
/// exercised end-to-end against a live model.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_run_with_store_leaves_no_dangling_checkpoint() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let agent = real_agent().await;
    let opts = SessionOptions::new()
        .with_session_id("real-ckpt-clear")
        .with_session_store(Arc::clone(&store));
    let session = agent
        .session_async("/tmp/real-ckpt-clear", Some(opts))
        .await
        .expect("session");

    let result = session
        .send(
            "Reply with the single word: done. Do not call any tools.",
            None,
        )
        .await
        .expect("real send should succeed");
    assert!(!result.text.is_empty());

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "one run recorded");
    let run_id = &runs[0].id;
    assert_eq!(runs[0].status, a3s_code_core::run::RunStatus::Completed);

    // Whether or not the model used a tool (which would have written a
    // checkpoint mid-run), the completed run must leave none behind.
    let lingering = store.load_loop_checkpoint(run_id).await.expect("load");
    assert!(
        lingering.is_none(),
        "completed real run must not leave a dangling loop checkpoint"
    );
}

/// Identity labels (tenant/principal/template/correlation) attached to a
/// session survive through a live run and the run is recorded as Completed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_identity_labels_survive_live_run() {
    let agent = real_agent().await;
    let opts = SessionOptions::new()
        .with_session_id("real-labels")
        .with_tenant_id("acme-prod")
        .with_principal("svc-bot")
        .with_agent_template_id("planner-v3")
        .with_correlation_id("trace-real-1");
    let session = agent
        .session_async("/tmp/real-labels", Some(opts))
        .await
        .expect("session");

    let result = session
        .send("Reply with the single word: ok", None)
        .await
        .expect("real send should succeed");
    assert!(!result.text.is_empty());

    assert_eq!(session.tenant_id(), Some("acme-prod"));
    assert_eq!(session.principal(), Some("svc-bot"));
    assert_eq!(session.agent_template_id(), Some("planner-v3"));
    assert_eq!(session.correlation_id(), Some("trace-real-1"));

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, a3s_code_core::run::RunStatus::Completed);
}

/// `resume_run` against a live model: seed a checkpoint carrying non-zero
/// cumulative metrics, resume, and confirm the run completes AND the
/// resumed AgentResult's usage is at least the seeded amount (i.e. metrics
/// carried forward, not reset to zero) plus the real turn's tokens.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_resume_run_carries_checkpoint_metrics_forward() {
    use a3s_code_core::llm::{ContentBlock, Message};
    use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let seeded_run = "real-resume-old";
    let seeded_total = 500u32;
    store
        .save_loop_checkpoint(
            seeded_run,
            &LoopCheckpoint {
                schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: seeded_run.to_string(),
                session_id: "real-resume".to_string(),
                turn: 1,
                messages: vec![
                    Message::user("Reply with the single word: ok"),
                    Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "working".to_string(),
                        }],
                        reasoning_content: None,
                    },
                ],
                total_usage: TokenUsage {
                    prompt_tokens: 400,
                    completion_tokens: 100,
                    total_tokens: seeded_total as usize,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                tool_calls_count: 2,
                verification_reports: Vec::new(),
                convergence: Default::default(),
                checkpoint_ms: 1_700_000_000_000,
            },
        )
        .await
        .expect("seed checkpoint");

    let agent = real_agent().await;
    let opts = SessionOptions::new()
        .with_session_id("real-resume")
        .with_session_store(Arc::clone(&store));
    let session = agent
        .session_async("/tmp/real-resume", Some(opts))
        .await
        .expect("session");

    let result = session
        .resume_run(seeded_run)
        .await
        .expect("resume_run against real model should succeed");

    assert!(!result.text.is_empty(), "resumed run produced text");
    assert!(
        result.usage.total_tokens > seeded_total as usize,
        "resumed usage ({}) must exceed the seeded {} (carried forward + real turn)",
        result.usage.total_tokens,
        seeded_total
    );
    assert!(
        result.tool_calls_count >= 2,
        "seeded tool-call count must carry forward"
    );
}
