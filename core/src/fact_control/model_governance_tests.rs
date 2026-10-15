#![cfg(test)]

use super::*;
use crate::agent::{FactModelAccountingObservation, FactOutputObservation, FactStoreObservation};
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmResponse, StreamEvent, TokenUsage};
use std::future::Future;

#[derive(Default)]
struct Records(Mutex<Vec<TokenUsage>>);
#[async_trait::async_trait]
impl BudgetGuard for Records {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push(usage.clone());
    }
}
struct Provider;
fn response() -> LlmResponse {
    LlmResponse {
        message: Message::assistant("actual parent response"),
        usage: TokenUsage {
            prompt_tokens: 17,
            completion_tokens: 13,
            total_tokens: 30,
            cache_read_tokens: Some(2),
            cache_write_tokens: None,
        },
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    }
}
#[async_trait::async_trait]
impl LlmClient for Provider {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("stream success must not fallback");
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.send(StreamEvent::Done(response())).await.unwrap();
        Ok(receiver)
    }
}
async fn surface() -> (
    tempfile::TempDir,
    SessionSurface,
    Arc<Records>,
    Arc<Provider>,
) {
    let workspace = tempfile::tempdir().unwrap();
    let guard = Arc::new(Records::default());
    let client = Arc::new(Provider);
    let agent = crate::agent::AgentLoop::new(
        client.clone(),
        Arc::new(crate::tools::ToolExecutor::new(
            workspace.path().display().to_string(),
        )),
        crate::tools::ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            budget_guard: Some(guard.clone()),
            ..Default::default()
        },
    )
    .with_model_generation_admission(crate::llm::ModelGenerationAdmission::default());
    let store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = store.create_run("parent-original", "parent prompt").await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let invocation = agent.invocation_context(
        run.id.clone(),
        Some("parent-original"),
        None,
        cancel.clone(),
    );
    let surface = SessionSurface {
        agent: invocation.bind_agent_loop(&agent),
        session_id: "parent-original".into(),
        fact_log: Default::default(),
        checkpoint: None,
        events: None,
        pending_memory_telemetry: Default::default(),
        streamed_text: Default::default(),
        forced_stop: Default::default(),
        cancel,
        transcript: Default::default(),
        usage: Default::default(),
        confirmation: None,
        run_store: Some(store),
        run_id: Some(run.id),
        goal_tick: None,
        ledger: Default::default(),
        reports: Default::default(),
        run_control: None,
        targeted_paths: Default::default(),
        harness: None,
        host_harness_registry: None,
        host_harness_assembler: None,
    };
    (workspace, surface, guard, client)
}

#[tokio::test]
async fn original_parent_projects_actual_input_usage_and_store_before_public_in_order() {
    let (_workspace, mut surface, guard, client) = surface().await;
    let (events, mut public) = tokio::sync::mpsc::channel(16);
    surface.events = Some(events);
    let prepared = surface
        .agent
        .prepare_fact_model_call(
            &surface.session_id,
            client,
            vec![Message::user("same actual input")],
            None,
            vec![],
        )
        .unwrap();
    let accounting = Arc::new(FactModelAccountingObservation::default());
    let (frames, mut receiver) = tokio::sync::mpsc::channel(64);
    let actual =
        tokio::spawn(prepared.run(frames, surface.cancel.child_token(), accounting.clone()));
    let mut done = None;
    while let Some(frame) = receiver.recv().await {
        if let Some(response) = project_model_frame(&surface, frame, &accounting)
            .await
            .unwrap()
        {
            done = Some(response);
        }
    }
    assert_eq!(
        actual.await.unwrap().unwrap().accounting,
        crate::agent::FactAccountingDisposition::CallbackReturned
    );
    assert_eq!(done.unwrap().usage.total_tokens, 30);
    let store = surface.run_store.as_ref().unwrap();
    let run_id = surface.run_id.as_deref().unwrap();
    let records = store.events(run_id).await;
    assert_eq!(records.len(), 4);
    let mut observed = Vec::new();
    for record in &records {
        let event = public.recv().await.unwrap();
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::to_value(&record.event).unwrap()
        );
        observed.push(event);
    }
    let capability = match &observed[0] {
        crate::agent::AgentEvent::RunCapabilityBound {
            snapshot,
            call_sequence,
        } => {
            assert_eq!(*call_sequence, 1);
            snapshot
        }
        other => panic!("actual first capability: {other:?}"),
    };
    let presentation = match &observed[1] {
        crate::agent::AgentEvent::ModelPresentationBound { snapshot } => snapshot,
        other => panic!("actual second presentation: {other:?}"),
    };
    let input = match &observed[2] {
        crate::agent::AgentEvent::ModelInputBound { snapshot } => snapshot,
        other => panic!("actual third input: {other:?}"),
    };
    let usage = match &observed[3] {
        crate::agent::AgentEvent::ModelUsageBound { snapshot } => snapshot,
        other => panic!("actual fourth usage: {other:?}"),
    };
    input.validate_against(capability).unwrap();
    presentation.validate_against(input).unwrap();
    usage.validate_against(input).unwrap();
    assert_eq!(guard.0.lock().unwrap().len(), 1);
    assert_eq!(surface.usage.lock().unwrap().total_tokens, 30);
    let state = accounting.usage_projection.lock().unwrap();
    assert!(state._queued);
    assert!(
        matches!(&state.store, FactStoreObservation::Returned(Some(snapshot)) if snapshot.id == run_id && snapshot.session_id == surface.session_id && snapshot.event_count == 4)
    );
    assert!(matches!(
        &state.output,
        FactOutputObservation::Returned(Ok(()))
    ));
}

#[tokio::test]
async fn actual_store_none_and_closed_output_are_retained_without_success_ack() {
    let (_workspace, mut missing, _, _) = surface().await;
    missing.run_id = Some("not-an-actual-run".into());
    let accounting = FactModelAccountingObservation::default();
    let result = project_model_frame(
        &missing,
        crate::agent::FactModelFrame::RawStream(StreamEvent::TextDelta("none".into())),
        &accounting,
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(
        &accounting.last_projection.lock().unwrap().store,
        FactStoreObservation::Returned(None)
    ));
    assert!(matches!(
        &accounting.last_projection.lock().unwrap().output,
        FactOutputObservation::NotConfigured
    ));
    let (_workspace2, mut closed, _, _) = surface().await;
    let (events, receiver) = tokio::sync::mpsc::channel(1);
    drop(receiver);
    closed.events = Some(events);
    let accounting = FactModelAccountingObservation::default();
    assert!(project_model_frame(
        &closed,
        crate::agent::FactModelFrame::RawStream(StreamEvent::TextDelta(
            "stored before failed send".into()
        )),
        &accounting
    )
    .await
    .is_err());
    {
        let state = accounting.last_projection.lock().unwrap();
        assert!(
            matches!(&state.store, FactStoreObservation::Returned(Some(snapshot)) if snapshot.event_count == 1)
        );
        assert!(matches!(
            &state.output,
            FactOutputObservation::Returned(Err(_))
        ));
    }
    assert_eq!(
        closed
            .run_store
            .as_ref()
            .unwrap()
            .events(closed.run_id.as_deref().unwrap())
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn dropping_actual_blocked_public_projection_keeps_real_store_some_and_entered_send() {
    let (_workspace, mut surface, _, _) = surface().await;
    let (events, mut receiver) = tokio::sync::mpsc::channel(1);
    events
        .send(crate::agent::AgentEvent::TextDelta {
            text: "real occupied public slot".into(),
        })
        .await
        .unwrap();
    surface.events = Some(events);
    let accounting = FactModelAccountingObservation::default();
    let mut actual = Box::pin(project_model_frame(
        &surface,
        crate::agent::FactModelFrame::RawStream(StreamEvent::TextDelta(
            "actual projected text".into(),
        )),
        &accounting,
    ));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(actual.as_mut().poll(cx).is_pending()))
            .await
    );
    {
        let state = accounting.last_projection.lock().unwrap();
        assert!(
            matches!(&state.store, FactStoreObservation::Returned(Some(snapshot)) if snapshot.event_count == 1)
        );
        assert!(matches!(&state.output, FactOutputObservation::Entered));
    }
    drop(actual);
    let records = surface
        .run_store
        .as_ref()
        .unwrap()
        .events(surface.run_id.as_deref().unwrap())
        .await;
    assert!(
        matches!(&records[0].event, crate::agent::AgentEvent::TextDelta { text } if text == "actual projected text")
    );
    assert!(
        matches!(receiver.recv().await, Some(crate::agent::AgentEvent::TextDelta { text }) if text == "real occupied public slot")
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        &accounting.last_projection.lock().unwrap().output,
        FactOutputObservation::Entered
    ));
    assert!(
        accounting.projection_has_missing_return(),
        "actual entered send blocks the model release conjunction"
    );
}
