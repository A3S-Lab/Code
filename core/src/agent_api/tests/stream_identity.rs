#![cfg(test)]

use super::*;
use crate::CodeError;

// Catch a fast worker settling before a host can query current_run for the stream it just admitted.
#[tokio::test]
async fn admitted_stream_identity_survives_fast_completion_and_later_runs() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            Arc::new(StaticStreamingClient::new("finished")),
            &SessionOptions::new().with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let mut first = session.stream_with_run("first", None).await.unwrap();
    let owner = first.run.take().expect("model execution admits a Run");
    while first.events.recv().await.is_some() {}
    first.worker.await.unwrap();
    assert!(session.current_run().await.is_none());
    let snapshot = owner.snapshot().await.unwrap();
    assert_eq!(snapshot.id, owner.id());
    assert_eq!(snapshot.session_id, session.session_id());
    assert_eq!(snapshot.prompt, "first");
    assert_eq!(snapshot.status, crate::run::RunStatus::Completed);
    assert!(snapshot.capability_binding.is_some());

    let mut second = session.stream_with_run("second", None).await.unwrap();
    let next = second.run.take().unwrap();
    assert_ne!(owner.id(), next.id());
    assert!(
        !owner.cancel().await,
        "old handle cannot cancel a later Run"
    );
    while second.events.recv().await.is_some() {}
    second.worker.await.unwrap();
    assert_eq!(
        next.snapshot().await.unwrap().status,
        crate::run::RunStatus::Completed
    );
    assert_eq!(owner.snapshot().await.unwrap().prompt, "first");
    session.close().await;
}

// Catch assigning a model Run to a local slash command or minting a stream identity after close.
#[tokio::test]
async fn commands_and_closed_sessions_do_not_mint_model_run_identities() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            Arc::new(StaticStreamingClient::new("must not become a model Run")),
            &SessionOptions::new(),
        )
        .unwrap();
    let mut command = session.stream_with_run("/help", None).await.unwrap();
    assert!(command.run.is_none());
    let mut output = String::new();
    while let Some(event) = command.events.recv().await {
        if let AgentEvent::TextDelta { text } = event {
            output.push_str(&text);
        }
    }
    command.worker.await.unwrap();
    assert!(output.contains("/help"));
    assert!(session.runs().await.is_empty());
    session.close().await;
    assert!(matches!(
        session.stream_with_run("later", None).await,
        Err(CodeError::SessionClosed { .. })
    ));
}
