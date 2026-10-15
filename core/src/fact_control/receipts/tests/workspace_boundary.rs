//! A path refusal decided before the tool body runs is a known terminal.
//! It must not book an unresolved external operation, and it must not
//! freeze a later tool or session completion.

use super::*;

#[tokio::test]
async fn known_workspace_boundary_refusal_does_not_block_a_later_tool_or_completion() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("refused.txt");
    let (tools, count) = executor_tools(root.path());
    let mut refused = request(
        "outside-write",
        json!({
            "file_path": outside_file.display().to_string(),
            "content": "must not land"
        }),
    );
    refused.call.name = "write".into();
    let prepared = prepare_tools(
        Arc::clone(&tools),
        refused,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    let refused_result = execute_prepared(Arc::clone(&tools), prepared).await;
    assert_ne!(
        refused_result.outcome,
        ToolExecutionOutcome::Unknown,
        "a pre-execution path refusal is a known terminal"
    );
    assert!(refused_result
        .effects
        .unresolved_external_operations
        .is_empty());
    assert_eq!(
        refused_result.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(!outside_file.exists());
    assert!(
        fold_debt(&[prepared_fact(&identity), result_fact(&refused_result)])
            .unresolved
            .is_empty()
    );
    assert!(!tools
        .ledger
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .has_uncertain_tool_invocations());

    let later = prepare_tools(
        Arc::clone(&tools),
        request("later-write", json!({"write": true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let later_result = execute_prepared(Arc::clone(&tools), later).await;
    assert_eq!(later_result.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.path().join("changed.txt")).unwrap(),
        b"exact bytes"
    );

    let ledger = tools.ledger.as_ref().unwrap().lock().unwrap().clone();
    assert!(!ledger.has_uncertain_tool_invocations());
    let report = crate::verification::VerificationReport::new(
        "synthetic byte check",
        vec![crate::verification::VerificationCheck::required(
            "exact-bytes",
            "file",
            "read changed.txt and compared its bytes",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(ledger.digest());
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[report], &[], false),
        crate::harness_loop::CompletionGate::Allow(_)
    ));
}
