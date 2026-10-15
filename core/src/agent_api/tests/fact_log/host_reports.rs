//! A host-recorded verification report has to reach the next fact-path
//! completion gate. Recording it only on the session store leaves the fresh
//! surface with an empty report list, so the same mutation digest stays open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use super::*;

struct WriteOnceClient {
    wrote: AtomicBool,
}

#[async_trait]
impl LlmClient for WriteOnceClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if tools.is_empty() {
            return Ok(text_response("[]"));
        }
        if self.wrote.swap(true, Ordering::SeqCst) {
            return Ok(text_response("stopped"));
        }
        Ok(tool_response(
            "write-host",
            "write",
            serde_json::json!({ "file_path": "noted.txt", "content": "noted" }),
        ))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("use complete")
    }
}

fn mutation_digest(message: &str) -> String {
    let Some((_, rest)) = message.split_once("workspace mutation ") else {
        return String::new();
    };
    rest.chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect()
}

#[tokio::test]
async fn a_host_recorded_report_closes_the_same_digest_on_the_next_stream() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let policy = crate::permissions::PermissionPolicy {
        default_decision: crate::permissions::PermissionDecision::Allow,
        ..Default::default()
    };
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(WriteOnceClient {
                wrote: AtomicBool::new(false),
            }),
            &crate::SessionOptions::new()
                .with_session_id("host-report-next-stream")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_permission_policy(policy),
        )
        .unwrap();

    let open = session
        .send("create noted.txt", None)
        .await
        .expect_err("an unbound mutation must not settle as success");
    let digest = mutation_digest(&open.to_string());
    assert_eq!(digest.len(), 64, "gate message was {}", open);
    assert_eq!(
        std::fs::read(workspace.path().join("noted.txt")).unwrap(),
        b"noted"
    );

    let report = crate::verification::VerificationReport::new(
        "host admission",
        vec![crate::verification::VerificationCheck::required(
            "check:host",
            "host",
            "the host bound this mutation digest",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(digest);
    session.record_verification_reports([report]);

    let settled = session
        .send("stop without another edit", None)
        .await
        .expect("the recorded report must satisfy the next completion gate");
    assert!(
        matches!(
            settled.completion,
            crate::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "completion was {:?}",
        settled.completion
    );
}
