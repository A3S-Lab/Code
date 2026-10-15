#![cfg(test)]

use super::*;
use a3s_effect::{
    Fact, ToolExecutionOutcome, ToolExecutionResultV1, ToolObservationCompleteness, ToolPreparedV1,
};

const SESSION_ID: &str = "lh4-plan-heavy-twin";
const BEFORE: &str = "export const A2 = 2;\n";
const AFTER: &str = "export const A2 = 4;\n";
const CHECKER: &str = "import assert from 'node:assert/strict';\n\
    import { A2 } from './src/impl.mjs';\n\
    assert.equal(A2, 4, 'PLAN_HEAVY_WRONG_VALUE');\n\
    console.log('PLAN_HEAVY_VERIFIED');\n";

/// Plan management is not evidence gathering. Six plan/read pairs must
/// reach a real edit and a real declared-suite run past the read-only
/// ceiling. A denied native command is not a passing verification.
struct PlanHeavyClient {
    calls: AtomicUsize,
    seen_systems: std::sync::Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for PlanHeavyClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.seen_systems
            .lock()
            .unwrap()
            .push(system.map(str::to_string));
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let message = if call < 12 {
            let block = if call.is_multiple_of(2) {
                tool_use(
                    &format!("ph-plan-{call}"),
                    "update_plan",
                    serde_json::json!({ "plan": [
                        { "step": "inspect", "status": "in_progress" },
                        { "step": "fix", "status": "pending" },
                        { "step": "verify", "status": "pending" },
                    ]}),
                )
            } else {
                tool_use(
                    &format!("ph-read-{call}"),
                    "read",
                    serde_json::json!({ "path": "data/a.mjs" }),
                )
            };
            assistant_tool(block)
        } else if call == 12 {
            assistant_tool(tool_use(
                "ph-edit",
                "edit",
                serde_json::json!({
                    "file_path": "src/impl.mjs",
                    "old_string": BEFORE,
                    "new_string": AFTER,
                }),
            ))
        } else if call == 13 {
            assistant_tool(tool_use(
                "ph-verify",
                "bash",
                serde_json::json!({ "command": "node test.mjs" }),
            ))
        } else {
            Message::assistant("PLAN_HEAVY_DONE")
        };
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

fn assistant_tool(block: crate::llm::ContentBlock) -> Message {
    Message {
        role: "assistant".into(),
        content: vec![block],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
    }
}

fn bound_receipt(facts: &[Fact], id: &str) -> ToolExecutionResultV1 {
    let results: Vec<_> = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result" && fact.payload["toolCallId"] == id)
        .collect();
    assert_eq!(results.len(), 1, "exactly one terminal receipt for {id}");
    let receipt: ToolExecutionResultV1 =
        serde_json::from_value(results[0].payload["receipt"].clone()).unwrap();
    let prepared: ToolPreparedV1 = facts
        .iter()
        .filter(|fact| fact.kind == "tool.prepared")
        .map(|fact| serde_json::from_value(fact.payload["prepared"].clone()).unwrap())
        .find(|prepared: &ToolPreparedV1| prepared.identity.tool_call_id == id)
        .expect("the terminal must have a matching prepared invocation");
    receipt.ensure_prepared(&prepared).unwrap();
    receipt
}

pub(super) async fn run_plan_heavy_choreography() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("data/a.mjs"), "export const A = 2;\n").unwrap();
    std::fs::write(root.join("src/impl.mjs"), BEFORE).unwrap();
    std::fs::write(root.join("test.mjs"), CHECKER).unwrap();

    let client = Arc::new(PlanHeavyClient {
        calls: AtomicUsize::new(0),
        seen_systems: std::sync::Mutex::new(Vec::new()),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id(SESSION_ID)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "plan-heavy-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    let result = session
        .send(
            "Plan and inspect before the first edit, then run the suite.",
            None,
        )
        .await;

    // Inspect actual effects even when completion fails, so a native
    // refusal cannot conceal the original invalid-edit regression.
    assert_eq!(
        std::fs::read_to_string(root.join("src/impl.mjs")).unwrap(),
        AFTER
    );
    assert_eq!(
        std::fs::read_to_string(root.join("test.mjs")).unwrap(),
        CHECKER
    );
    let systems = client.seen_systems.lock().unwrap().clone();
    assert!(!systems.is_empty());
    for system in &systems {
        assert!(
            !system
                .as_deref()
                .is_some_and(|text| text.contains("Read-only evidence gathering")),
            "plan-heavy choreography must not trip the read-only ceiling: {system:?}"
        );
    }
    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session(SESSION_ID),
    )
    .unwrap();
    let count = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .count();
    assert!(
        count >= 14,
        "12 inspect results, an edit and a checker; got {count}"
    );
    let edit = bound_receipt(&facts, "ph-edit");
    assert_eq!(edit.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(
        edit.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(edit
        .effects
        .changes
        .iter()
        .any(|change| change.path == "src/impl.mjs"));
    let verify = bound_receipt(&facts, "ph-verify");
    assert_eq!(
        verify.outcome,
        ToolExecutionOutcome::Succeeded,
        "real checker required: {verify:?}"
    );
    assert_eq!(verify.exit_code, Some(0));
    assert_eq!(
        verify.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(verify.output.to_string().contains("PLAN_HEAVY_VERIFIED"));

    let result = result.expect("the real passing checker must complete the mutation gate");
    let crate::harness_loop::CompletionTerminal::Verified { effect_digest } = &result.completion
    else {
        panic!(
            "plan-heavy edit must complete Verified: {:?}",
            result.completion
        );
    };
    assert!(result
        .verification_reports
        .iter()
        .any(|report| report.subject == "shell:contract:plan-heavy-suite"
            && report.effect_digest.as_ref() == Some(effect_digest)
            && report.checks.iter().any(|check| check.required
                && check.status == crate::verification::VerificationStatus::Passed)));
}
