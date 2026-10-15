#![cfg(test)]

use super::*;

#[tokio::test]
async fn interactive_delivery_calls_the_model_after_verification_without_replaying_tools() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(
        crate::tools::builtin::background::test_backend::DeterministicBackgroundSandbox::new(
            workspace.path().to_path_buf(),
            "protocol-file-exists",
            0,
            false,
        ),
    );
    let coding_prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(VerifiedThenTextClient {
        coding_prompts: Arc::clone(&coding_prompts),
        coding_calls: AtomicUsize::new(0),
    });
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::clone(&client) as Arc<dyn LlmClient>,
            &crate::SessionOptions::new()
                .with_session_id("verified-assistant-delivery")
                .with_assistant_final_response(true)
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::BashSandbox>)
                .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let result = session
        .send("write hello.txt, verify it, then answer", None)
        .await
        .unwrap();
    assert_eq!(
        result.text, "second-turn",
        "deliver the actual assistant reply, not synthesized completed"
    );
    assert!(matches!(
        result.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));
    assert_eq!(client.coding_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        sandbox.calls(),
        1,
        "verification is not repeated to obtain the reply"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).unwrap(),
        "hello"
    );
    assert!(coding_prompts
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("hello.txt"));
    let facts = read_workspace_facts(workspace.path(), "verified-assistant-delivery").unwrap();
    assert!(facts
        .iter()
        .any(|fact| fact.kind == "turn.complete" && fact.payload["terminal"] == "verified"));
}

#[tokio::test]
async fn assistant_delivery_cannot_replace_missing_verification() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = session_for(
        &agent,
        workspace.path(),
        "unverified-assistant-delivery",
        Arc::new(ScriptedClient {
            seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            responses: std::sync::Mutex::new(vec![
                tool_response(
                    "write",
                    "write",
                    serde_json::json!({"file_path":"hello.txt", "content":"hello"}),
                ),
                text_response("all checked"),
            ]),
        }),
        &crate::SessionOptions::new()
            .with_assistant_final_response(true)
            .with_max_tool_rounds(3)
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*")),
    );
    let error = session
        .send("write hello.txt", None)
        .await
        .expect_err("text cannot verify a mutation");
    assert!(
        error.to_string().contains("no bound Passed verification"),
        "{error}"
    );
    let facts = read_workspace_facts(workspace.path(), "unverified-assistant-delivery").unwrap();
    assert!(!facts
        .iter()
        .any(|fact| fact.kind == "turn.complete" && fact.payload["terminal"] == "verified"));
}
