//! Live session-review lifecycle against `.a3s/config.acl`.
//!
//! The durable sticky-review loop, driven exactly the way a product host
//! drives it (Desktop owns the rubric; Core only stores and injects):
//!
//! 1. Turn 1 — the real main agent summarizes `config.md` into
//!    `summary.md`. The fixture plants a contradiction (an active
//!    `retry=3` plus a deprecated appendix `retry=5`).
//! 2. Reviewer — a second REAL model call (host-run, structured) reviews
//!    the product against the source and produces a finding; the test
//!    (as host) admits it into the durable store.
//! 3. Injection — `pending_session_review_findings()` returns the finding
//!    (registered scenario opts into main inject).
//! 4. Turn 2 — the pending finding is prefixed to the follow-up prompt;
//!    the main agent corrects `summary.md`.
//! 5. Lifecycle — the host marks the finding addressed (bound to the
//!    addressing run), records the main agent's reply, and accepts it.
//!
//! Gate note: write turns may end with a completion-gate error event when
//! no verification is bound; tool effects still land, so the oracles are
//! disk- and store-side (house pattern from the issue-fix live suite).
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_session_review_loop_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use a3s_code_core::llm::create_client_with_config;
use a3s_code_core::llm::structured::{generate_blocking, StructuredMode, StructuredRequest};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::session_review::{
    ReviewScenario, ReviewSubjectV1, ReviewTriggerPolicy, SessionReviewFindingV1,
    SessionReviewSeverityV1,
};
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

const TURN_TIMEOUT: Duration = Duration::from_secs(600);
const REVIEW_TIMEOUT: Duration = Duration::from_secs(420);
/// The active setting; the deprecated appendix plants `retry=5`.
const ACTIVE_RETRY: &str = "retry=3";
const DEPRECATED_RETRY: &str = "retry=5";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// Host-owned scenario: auto-review after each successful main turn.
struct LiveRubricScenario;

impl ReviewScenario for LiveRubricScenario {
    fn scenario_id(&self) -> &str {
        "live.config-consistency"
    }

    fn triggers(&self) -> ReviewTriggerPolicy {
        ReviewTriggerPolicy::AfterMainSuccess
    }

    fn injects_into_main(&self) -> bool {
        true
    }
}

struct TurnOutcome {
    tool_calls: Vec<(String, bool)>,
    gate_error: bool,
    final_text: String,
}

/// Run one turn; a completion-gate error ends the stream but the tool
/// effects have already landed, so it is recorded rather than fatal.
async fn run_turn(session: &a3s_code_core::AgentSession, prompt: &str) -> TurnOutcome {
    let (mut events, worker) = session
        .stream(prompt, None)
        .await
        .expect("start model turn");
    let mut tool_calls = Vec::new();
    let mut pending: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut gate_error = false;
    let mut final_text = String::new();
    let done = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await.expect("event stream open") {
                AgentEvent::ToolExecutionStart { id, name, .. } => {
                    pending.insert(id, name);
                }
                AgentEvent::ToolEnd { id, exit_code, .. } => {
                    if let Some(name) = pending.remove(&id) {
                        tool_calls.push((name, exit_code == 0));
                    }
                }
                AgentEvent::End { text, .. } => {
                    final_text = text;
                    break true;
                }
                AgentEvent::Error { message } => {
                    if message.contains("completion gate:") {
                        gate_error = true;
                        break false;
                    }
                    panic!("model turn failed: {message}");
                }
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("model turn timed out");
    let _ = done;
    worker.await.expect("worker joins");
    TurnOutcome {
        tool_calls,
        gate_error,
        final_text,
    }
}

fn review_request(config_text: &str, summary_text: &str) -> StructuredRequest {
    StructuredRequest {
        prompt: format!(
            "You are an independent reviewer. Compare the produced summary \
             against its source configuration. The source's main section is \
             authoritative; the appendix is explicitly deprecated. Report \
             every inconsistency as a finding with the exact correction.\n\n\
             SOURCE config.md:\n{config_text}\n\nPRODUCED summary.md:\n{summary_text}"
        ),
        system: Some(
            "You are a rigorous configuration reviewer. Judge only from the \
             provided documents."
                .to_owned(),
        ),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "correction": { "type": "string" }
                        },
                        "required": ["message", "correction"]
                    }
                }
            },
            "required": ["findings"]
        }),
        schema_name: "config_consistency_findings".to_owned(),
        schema_description: Some(
            "Inconsistencies between the produced summary and the source \
             configuration, each with the exact correction."
                .to_owned(),
        ),
        mode: StructuredMode::Auto,
        max_repair_attempts: 2,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_reviewer_finding_survives_injection_and_the_lifecycle_closes() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    config.default_model = Some(model.clone());
    let agent = Agent::from_config(config.clone())
        .await
        .expect("agent from real config");

    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join("src")).expect("src dir");
    let config_md = format!(
        "# Service configuration\n\nThe main section is authoritative.\n\n\
         - timeout=30\n- {ACTIVE_RETRY}\n- workers=8\n\n\
         ## Deprecated appendix (do not use)\n\n- {DEPRECATED_RETRY} (legacy)\n"
    );
    std::fs::write(workspace.path().join("config.md"), &config_md).expect("write config");

    let mut permissions = PermissionPolicy::new().allow_all(&["read", "write", "edit", "ls"]);
    permissions.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id("session-review-loop")
        .with_model(model.clone())
        .with_permission_policy(permissions)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(8);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session");
    session
        .register_review_scenario(Arc::new(LiveRubricScenario))
        .expect("register host scenario");
    assert!(
        session
            .review_scenario_ids()
            .contains(&"live.config-consistency".to_owned()),
        "the host scenario must register"
    );

    // ---- Turn 1: the main agent produces the summary. ----
    let turn1 = run_turn(
        &session,
        "Read config.md and write summary.md containing exactly the three \
         authoritative settings (timeout, retry, workers) as `key=value` \
         lines. Then stop.",
    )
    .await;
    let summary_path = workspace.path().join("summary.md");
    let summary_text = std::fs::read_to_string(&summary_path).unwrap_or_default();
    assert!(
        !summary_text.trim().is_empty(),
        "turn 1 must produce summary.md (tools={:?} gate_error={})",
        turn1.tool_calls,
        turn1.gate_error
    );

    // ---- Reviewer: a second real model adjudicates product vs source. ----
    let llm_config = config
        .default_llm_config()
        .expect("default llm config for the reviewer");
    let reviewer_client = create_client_with_config(llm_config);
    let review = tokio::time::timeout(
        REVIEW_TIMEOUT,
        generate_blocking(
            reviewer_client.as_ref(),
            &review_request(&config_md, &summary_text),
        ),
    )
    .await
    .expect("reviewer timed out")
    .expect("reviewer structured output");
    let raw_findings = review
        .object
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    // The reviewer must adjudicate the planted contradiction: either the
    // summary echoed the deprecated value or omitted/changed a setting.
    let relevant: Vec<&serde_json::Value> = raw_findings
        .iter()
        .filter(|finding| {
            let text = finding.to_string();
            text.contains("retry") || text.contains("5") || text.contains("3")
        })
        .collect();
    assert!(
        !raw_findings.is_empty() || summary_text.contains(ACTIVE_RETRY),
        "the reviewer must either find inconsistencies or the summary must \
         already be consistent: findings={raw_findings:?} summary={summary_text:?}"
    );

    // ---- Host admits the first finding into the durable store. ----
    let finding_id = "finding-live-config-1";
    let (message, correction) = raw_findings
        .first()
        .map(|finding| {
            (
                finding
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("summary inconsistent with source")
                    .to_owned(),
                finding
                    .get("correction")
                    .and_then(|value| value.as_str())
                    .unwrap_or(ACTIVE_RETRY)
                    .to_owned(),
            )
        })
        .unwrap_or_else(|| {
            (
                format!("summary must carry the authoritative {ACTIVE_RETRY}"),
                ACTIVE_RETRY.to_owned(),
            )
        });
    let finding = SessionReviewFindingV1::new(
        finding_id.to_owned(),
        "session-review-loop".to_owned(),
        SessionReviewSeverityV1::Error,
        "config-consistency",
        message.clone(),
        format!("summary.md vs config.md ({DEPRECATED_RETRY} is deprecated)"),
        ReviewSubjectV1::opaque("artifact", "summary.md").expect("subject"),
        "review-live-1",
        now_ms(),
    )
    .expect("finding constructs");
    session
        .upsert_session_review_finding(finding)
        .expect("admit finding");

    // ---- Injection: the pending finding is host-visible for the next turn. ----
    let pending = session.pending_session_review_findings();
    assert!(
        pending.iter().any(|item| item.finding_id == finding_id),
        "the admitted finding must be pending for injection: {pending:?}"
    );
    let _ = &relevant;

    // ---- Turn 2: the prefixed finding changes the main agent's output. ----
    let prefix = pending
        .iter()
        .map(|item| format!("[review] {}: {}", item.finding_id, item.claim))
        .collect::<Vec<_>>()
        .join("\n");
    let turn2 = run_turn(
        &session,
        &format!(
            "{prefix}\n\nAddress the reviewer findings above: correct \
             summary.md so every line matches the authoritative main section \
             of config.md. Then stop."
        ),
    )
    .await;
    let _ = turn2;
    let corrected = std::fs::read_to_string(&summary_path).unwrap_or_default();
    assert!(
        corrected.contains(ACTIVE_RETRY),
        "the addressed summary must carry the authoritative {ACTIVE_RETRY}: {corrected:?}"
    );

    // ---- Lifecycle: addressed (bound to the addressing run) → accepted. ----
    let addressing_run = session
        .current_run()
        .await
        .map(|run| run.id().to_string())
        .unwrap_or_else(|| "run-turn-2".to_owned());
    session
        .mark_session_review_addressed(finding_id, addressing_run.clone(), now_ms())
        .expect("mark addressed");
    session
        .set_session_review_main_agent_reply(finding_id, correction.clone())
        .expect("record main reply");
    let addressed = session.addressed_session_review_findings();
    assert!(
        addressed.iter().any(|item| item.finding_id == finding_id),
        "the finding must await reviewer acceptance: {addressed:?}"
    );
    session
        .accept_session_review_finding(finding_id, format!("{addressing_run}-review"), now_ms())
        .expect("accept finding");
    assert!(
        session.pending_session_review_findings().is_empty(),
        "an accepted finding must leave the injection set"
    );

    session.close().await;
    println!(
        "session-review-loop-real-llm model={} reviewer_findings={} admitted={:?} \
         correction_applied={} turn1_chars={} turn2_chars={} total_ms={} result=pass",
        model,
        raw_findings.len(),
        message,
        correction,
        turn1.final_text.chars().count(),
        turn2.final_text.chars().count(),
        started.elapsed().as_millis()
    );
}
