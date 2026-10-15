#![cfg(test)]

use super::*;
use crate::goal::{standing_goal_prompt, GoalOp};
use a3s_code_core::{Agent, SessionOptions};

async fn agent() -> Agent {
    Agent::from_config(crate::agent::test_launch(None).config)
        .await
        .unwrap()
}

async fn session(agent: &Agent, root: &Path, id: &str) -> AgentSession {
    agent
        .session_async(
            root.display().to_string(),
            Some(SessionOptions::new().with_session_id(id)),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn session_goals_isolate_siblings_preserve_workspace_source_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let first = session(&agent, root.path(), "first").await;
    let second = session(&agent, root.path(), "second").await;
    let unowned = root.path().join(".a3s/durable-goal.json");
    let original = r#"{"objective":"explicit import only","paused":false}"#;
    std::fs::write(&unowned, original).unwrap();
    assert!(load_goal(root.path(), "first").await.unwrap().is_none());
    let applied = apply_session_goal(&first, GoalOp::Set("First objective".into()))
        .await
        .unwrap();
    assert!(applied.model_prompt.is_some());
    apply_session_goal(&second, GoalOp::Set("Second objective".into()))
        .await
        .unwrap();
    let file = goal_file(first.workspace(), "first").unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(apply_session_goal(&first, GoalOp::Status)
        .await
        .unwrap()
        .model_prompt
        .is_none());
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "status is read-only");
    let paused = apply_session_goal(&first, GoalOp::Pause)
        .await
        .unwrap()
        .next
        .unwrap();
    assert!(paused.paused);
    assert_eq!(standing_goal_prompt(Some(&paused), "hello"), "hello");
    assert_eq!(
        load_goal(second.workspace(), "second")
            .await
            .unwrap()
            .unwrap()
            .objective,
        "Second objective"
    );
    first.close().await;
    second.close().await;
    agent.close().await;
    let next_agent = self::agent().await;
    let first = session(&next_agent, root.path(), "first").await;
    assert_eq!(
        load_goal(first.workspace(), "first").await.unwrap(),
        Some(paused)
    );
    let resumed = apply_session_goal(&first, GoalOp::Resume).await.unwrap();
    assert!(resumed.model_prompt.unwrap().contains("First objective"));
    assert!(standing_goal_prompt(resumed.next.as_ref(), "next").contains("First objective"));
    apply_session_goal(&first, GoalOp::Clear).await.unwrap();
    assert!(load_goal(first.workspace(), "first")
        .await
        .unwrap()
        .is_none());
    assert_eq!(std::fs::read_to_string(&unowned).unwrap(), original);
    assert_eq!(
        load_goal(root.path(), "second")
            .await
            .unwrap()
            .unwrap()
            .objective,
        "Second objective"
    );
    first.close().await;
    next_agent.close().await;
}

#[tokio::test]
async fn retirement_requires_exact_active_governing_goal_and_settled_owner() {
    let root = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let session = session(&agent, root.path(), "retirement").await;
    let first = apply_session_goal(&session, GoalOp::Set("Ship release".into()))
        .await
        .unwrap()
        .next
        .unwrap();
    assert!(!retire_achieved_durable_goal(&session, None).await.unwrap());
    let paused = apply_session_goal(&session, GoalOp::Pause)
        .await
        .unwrap()
        .next
        .unwrap();
    assert!(!retire_achieved_durable_goal(&session, Some(&paused))
        .await
        .unwrap());
    assert!(!retire_achieved_durable_goal(&session, Some(&first))
        .await
        .unwrap());
    let second = apply_session_goal(&session, GoalOp::Set("New objective".into()))
        .await
        .unwrap()
        .next
        .unwrap();
    assert!(!retire_achieved_durable_goal(&session, Some(&first))
        .await
        .unwrap());
    session
        .with_legacy_goal_state(|| async {
            assert!(retire_achieved_durable_goal(&session, Some(&second))
                .await
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        load_goal(session.workspace(), "retirement").await.unwrap(),
        Some(second.clone())
    );
    assert!(retire_achieved_durable_goal(&session, Some(&second))
        .await
        .unwrap());
    assert!(load_goal(session.workspace(), "retirement")
        .await
        .unwrap()
        .is_none());
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn malformed_session_goal_is_not_silently_replaced_or_executed() {
    let root = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let session = session(&agent, root.path(), "invalid").await;
    let path = goal_file(session.workspace(), session.session_id()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not a goal").unwrap();
    for op in [
        GoalOp::Status,
        GoalOp::Resume,
        GoalOp::Set("replacement".into()),
        GoalOp::Clear,
    ] {
        assert!(apply_session_goal(&session, op).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a goal");
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(load_goal(session.workspace(), session.session_id())
        .await
        .is_err());
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn legacy_log_session_keeps_its_original_fact_directory() {
    let root = tempfile::tempdir().unwrap();
    let original = a3s_code_core::session_folder::legacy_effect_log_dir(root.path());
    std::fs::create_dir_all(&original).unwrap();
    std::fs::write(original.join("legacy-owner.jsonl"), "").unwrap();
    let agent = agent().await;
    let session = session(&agent, root.path(), "legacy-owner").await;
    apply_session_goal(&session, GoalOp::Set("Keep original source".into()))
        .await
        .unwrap();
    assert!(
        a3s_code_core::session_folder::find_session_dir(root.path(), "legacy-owner")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        a3s_code_core::session_folder::effect_log_dir(root.path(), "legacy-owner"),
        original
    );
    assert!(original.join("legacy-owner.durable-goal.json").is_file());
    assert!(std::fs::read(original.join("legacy-owner.jsonl"))
        .unwrap()
        .is_empty());
    assert!(!root.path().join(".a3s/durable-goal.json").exists());
    session.close().await;
    agent.close().await;
}
