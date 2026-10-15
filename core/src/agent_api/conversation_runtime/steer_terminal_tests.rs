#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use crate::run_control::{RunControlReceiptState, SteerRequest};
use tokio::sync::Semaphore;

const INITIAL: &str = "Finish the current answer.";
const FOLLOW_UP: &str = "Use the new workspace requirement on the next turn.";

struct FinalCallback {
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait::async_trait]
impl BudgetGuard for FinalCallback {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, _: &TokenUsage) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}

#[derive(Default)]
struct FinalAnswer(
    std::sync::atomic::AtomicUsize,
    std::sync::Mutex<Vec<Vec<Message>>>,
);
#[async_trait::async_trait]
impl LlmClient for FinalAnswer {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("successful actual Done must not fall back");
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.1.lock().unwrap().push(messages.to_vec());
        let (sender, receiver) = mpsc::channel(1);
        sender
            .try_send(StreamEvent::Done(LlmResponse {
                message: Message::assistant("The original answer is complete."),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: vec![],
                meta: None,
            }))
            .unwrap();
        Ok(receiver)
    }
}

#[tokio::test]
async fn accepted_steer_after_last_safe_point_is_retained_before_normal_completion() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let callback = Arc::new(FinalCallback {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let client = Arc::new(FinalAnswer::default());
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &crate::SessionOptions::new()
                    .with_session_id("terminal-steer")
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_budget_guard(callback.clone()),
            )
            .unwrap(),
    );
    let caller = session.clone();
    let send = tokio::spawn(async move { caller.send(INITIAL, None).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        callback.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let run = session.current_run().await.unwrap();
    let mut request = SteerRequest::new(FOLLOW_UP).with_run_id(run.id());
    request.request_id = Some("terminal-steer-request".into());
    let retry = request.clone().into_protocol("terminal-steer", run.id());
    let accepted = session.steer(request).await.unwrap();
    assert_eq!(accepted.state, RunControlReceiptState::Accepted);
    let inbox = RunControlState::from_session(&session)
        .current_run_control()
        .await
        .unwrap();
    callback.release.add_permits(1);
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), send)
        .await
        .unwrap()
        .unwrap();
    let facts = crate::fact_control::read_workspace_facts(
        workspace.path(),
        &crate::fact_control::thread_for_session("terminal-steer"),
    )
    .unwrap();
    let receipt = inbox.submit(retry, 1).await.unwrap();
    eprintln!("outcome={outcome:?} receipt={receipt:?} facts={facts:?}");
    assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        outcome.is_ok(),
        "ending Run must retain input and finish: {outcome:?}"
    );
    assert_eq!(receipt.state, RunControlReceiptState::Applied);
    let follow_up = facts
        .iter()
        .position(|fact| fact.kind == "user.message" && fact.payload["text"] == FOLLOW_UP)
        .expect("accepted exact input must be committed");
    let complete = facts
        .iter()
        .position(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND)
        .expect("actual completion marker");
    assert!(follow_up < complete);
}

async fn late_input_scenario(route: &str, file_store: bool) {
    use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
    let workspace = tempfile::tempdir().unwrap();
    let stores = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(stores.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let callback = Arc::new(FinalCallback {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let client = Arc::new(FinalAnswer::default());
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &crate::SessionOptions::new()
                    .with_session_id("late-input-matrix")
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_budget_guard(callback.clone())
                    .with_session_store(store.clone())
                    .with_auto_save(true),
            )
            .unwrap(),
    );
    let attachments = route == "attachments";
    let saves_history = route != "detached";
    let route = route.to_owned();
    let caller = session.clone();
    let send = tokio::spawn(async move {
        let attachment = Attachment::png(vec![1, 2, 3]);
        match route.as_str() {
            "send" => Some(caller.send(INITIAL, None).await.unwrap()),
            "attachments" => Some(
                caller
                    .send_with_attachments(INITIAL, &[attachment], None)
                    .await
                    .unwrap(),
            ),
            "stream" => {
                let mut actual = caller.stream_with_run(INITIAL, None).await.unwrap();
                let mut ends = 0;
                while let Some(event) = actual.events.recv().await {
                    assert!(!matches!(event, AgentEvent::Error { .. }), "{event:?}");
                    ends += usize::from(matches!(event, AgentEvent::End { .. }));
                }
                actual.worker.await.unwrap();
                assert_eq!(ends, 1);
                None
            }
            "detached" => {
                let AgentRunSpawn::Started { worker, .. } = caller
                    .spawn_run_with_id("external-late-run", INITIAL)
                    .await
                    .unwrap()
                else {
                    panic!("fresh external identity replayed");
                };
                worker.await.unwrap();
                None
            }
            _ => panic!("unknown test route"),
        }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        callback.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let run = session.current_run().await.unwrap();
    let input = "下一轮使用这些约束\n\t  ";
    let mut request = SteerRequest::new(input).with_run_id(run.id());
    request.request_id = Some("late-input-exact-request".into());
    let retry = request.clone().into_protocol("late-input-matrix", run.id());
    assert_eq!(
        session.steer(request).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
    let inbox = RunControlState::from_session(&session)
        .current_run_control()
        .await
        .unwrap();
    callback.release.add_permits(1);
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), send)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        inbox.submit(retry, 1).await.unwrap().state,
        RunControlReceiptState::Applied
    );
    let original = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
        workspace.path(),
        "late-input-matrix",
    ))
    .unwrap();
    let facts = a3s_effect::LogStore::read(&original, "late-input-matrix").unwrap();
    let retained = facts
        .iter()
        .position(|fact| fact.kind == "user.message" && fact.payload["text"] == input)
        .unwrap();
    let marker = facts
        .iter()
        .position(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND)
        .unwrap();
    assert!(retained < marker);
    if let Some(result) = result {
        assert!(result
            .messages
            .iter()
            .any(|message| message.role == "user" && message.text() == input));
        let had_image = client.1.lock().unwrap()[0].iter().any(|message| message.content.iter().any(|block| matches!(block, crate::llm::ContentBlock::Image { source } if source.data == "AQID")));
        if attachments {
            assert!(
                had_image,
                "original attachment must reach the actual provider"
            );
            assert!(result.messages.iter().any(|message| message.content.iter().any(|block| matches!(block, crate::llm::ContentBlock::Image { source } if source.data == "AQID"))));
        }
    }
    assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    session.save().await.unwrap();
    let reopened: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(stores.path()).await.unwrap())
    } else {
        store
    };
    let snapshot = reopened
        .load_snapshot("late-input-matrix")
        .await
        .unwrap()
        .unwrap();
    if saves_history {
        assert!(snapshot
            .session
            .messages
            .iter()
            .any(|message| message.role == "user" && message.text() == input));
    }
    callback.release.add_permits(1);
    session
        .send("Continue with the retained requirement.", None)
        .await
        .unwrap();
    assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(client.1.lock().unwrap()[1]
        .iter()
        .any(|message| message.role == "user" && message.text() == input));
    session.close().await;
}

#[tokio::test]
async fn late_input_reaches_original_log_store_and_next_provider_across_public_routes() {
    for route in ["send", "attachments", "stream", "detached"] {
        for file in [false, true] {
            eprintln!("late input route={route} file={file}");
            late_input_scenario(route, file).await;
        }
    }
}

#[tokio::test]
async fn actual_original_file_io_failure_retains_both_errors_and_never_publishes_success() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let callback = Arc::new(FinalCallback {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let client = Arc::new(FinalAnswer::default());
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &crate::SessionOptions::new()
                    .with_session_id("steer-file-fault")
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_budget_guard(callback.clone()),
            )
            .unwrap(),
    );
    let caller = session.clone();
    let send = tokio::spawn(async move { caller.send(INITIAL, None).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        callback.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let run = session.current_run().await.unwrap();
    let mut request = SteerRequest::new(FOLLOW_UP).with_run_id(run.id());
    request.request_id = Some("file-fault-exact-request".into());
    let retry = request.clone().into_protocol("steer-file-fault", run.id());
    assert_eq!(
        session.steer(request).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
    let inbox = RunControlState::from_session(&session)
        .current_run_control()
        .await
        .unwrap();
    let directory = crate::session_folder::effect_log_dir(workspace.path(), "steer-file-fault");
    let path = directory.join("steer-file-fault.jsonl");
    let bytes = std::fs::read(&path).unwrap();
    let retained = directory.join("original-before-fault.evidence");
    std::fs::rename(&path, &retained).unwrap();
    std::fs::create_dir(&path).unwrap();
    callback.release.add_permits(1);
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), send)
        .await
        .unwrap()
        .unwrap();
    let error = result.unwrap_err().to_string();
    eprintln!("actual combined failure={error}");
    assert!(error.contains("terminal input retention also failed"));
    assert!(error.contains("terminal transcript also failed"));
    let receipt = inbox.submit(retry, 1).await.unwrap();
    assert_eq!(receipt.state, RunControlReceiptState::Settled);
    assert_eq!(receipt.error.unwrap().code, "STEER_PERSISTENCE_FAILED");
    assert!(path.is_dir());
    assert_eq!(std::fs::read(&retained).unwrap(), bytes);
    let events = session.run_store.events(run.id()).await;
    assert!(events
        .iter()
        .any(|event| matches!(event.event, AgentEvent::Error { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    let prefix = String::from_utf8(bytes).unwrap();
    assert!(!prefix.contains("turn.complete"));
    assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    session.close().await;
}
