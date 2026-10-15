#![cfg(test)]

use super::*;
use std::collections::VecDeque;
use tokio::sync::Semaphore;

fn usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 17,
        completion_tokens: 13,
        total_tokens: 30,
        cache_read_tokens: Some(2),
        cache_write_tokens: None,
    }
}
fn response() -> LlmResponse {
    LlmResponse {
        message: Message::assistant("actual governed response"),
        usage: usage(),
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    }
}
fn tuple(value: &TokenUsage) -> (usize, usize, usize, Option<usize>, Option<usize>) {
    (
        value.prompt_tokens,
        value.completion_tokens,
        value.total_tokens,
        value.cache_read_tokens,
        value.cache_write_tokens,
    )
}

struct Guard {
    decisions: Mutex<VecDeque<BudgetDecision>>,
    estimates: Mutex<Vec<usize>>,
    records: Mutex<Vec<TokenUsage>>,
    entered: Semaphore,
    release: Semaphore,
    block: bool,
}
impl Guard {
    fn new(decisions: Vec<BudgetDecision>, block: bool) -> Arc<Self> {
        Arc::new(Self {
            decisions: Mutex::new(decisions.into()),
            estimates: Mutex::new(vec![]),
            records: Mutex::new(vec![]),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            block,
        })
    }
}
#[async_trait]
impl BudgetGuard for Guard {
    async fn check_before_llm(&self, session: &str, estimated: usize) -> BudgetDecision {
        assert_eq!(session, "fact-original");
        self.estimates.lock().unwrap().push(estimated);
        self.decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(BudgetDecision::Allow)
    }
    async fn record_after_llm(&self, session: &str, actual: &TokenUsage) {
        assert_eq!(session, "fact-original");
        self.records.lock().unwrap().push(actual.clone());
        // A real partial effect of the actual callback precedes this barrier.
        self.entered.add_permits(1);
        if self.block {
            self.release.acquire().await.unwrap().forget();
        }
    }
}

struct Client {
    receiver: Mutex<Option<mpsc::Receiver<StreamEvent>>>,
    setup_error: bool,
    terminal: bool,
    streaming: AtomicU64,
    complete: AtomicU64,
}
#[async_trait]
impl LlmClient for Client {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.complete.fetch_add(1, Ordering::SeqCst);
        Ok(response())
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.streaming.fetch_add(1, Ordering::SeqCst);
        if self.terminal {
            return Err(
                crate::llm::NonRetryableLlmError::new("terminal controlled provider").into(),
            );
        }
        if self.setup_error {
            anyhow::bail!("controlled retryable setup failure");
        }
        self.receiver
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("original receiver already taken"))
    }
}

fn fixture(
    guard: Arc<Guard>,
    setup_error: bool,
    terminal: bool,
) -> (AgentLoop, Arc<Client>, mpsc::Sender<StreamEvent>) {
    let (sender, receiver) = mpsc::channel(128);
    let client = Arc::new(Client {
        receiver: Mutex::new(Some(receiver)),
        setup_error,
        terminal,
        streaming: AtomicU64::new(0),
        complete: AtomicU64::new(0),
    });
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(crate::tools::ToolExecutor::new("/tmp".into())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig {
            budget_guard: Some(guard),
            ..Default::default()
        },
    )
    .with_model_generation_admission(ModelGenerationAdmission::default());
    let original = agent.invocation_context(
        "original-fact-run",
        Some("fact-original"),
        None,
        CancellationToken::new(),
    );
    (original.bind_agent_loop(&agent), client, sender)
}
fn prepared(agent: &AgentLoop, client: Arc<dyn LlmClient>) -> PreparedFactModelCall {
    agent
        .prepare_fact_model_call(
            "fact-original",
            client,
            vec![Message::user("original actual prompt")],
            None,
            vec![],
        )
        .unwrap()
}
fn send_static<F: Future + Send + 'static>(future: F) -> F {
    future
}
async fn callback_entered(guard: &Guard) {
    tokio::time::timeout(Duration::from_secs(5), guard.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
async fn actual_done_without_eof_records_once_even_when_parent_cancels_partial_callback() {
    let guard = Guard::new(vec![], true);
    let (agent, client, provider_sender) = fixture(guard.clone(), false, false);
    provider_sender
        .send(StreamEvent::Done(response()))
        .await
        .unwrap();
    let attempt = CancellationToken::new();
    let observation = Arc::new(FactModelAccountingObservation::default());
    let (frames, mut receiver) = mpsc::channel(64);
    let actual = tokio::spawn(send_static(prepared(&agent, client.clone()).run(
        frames,
        attempt.clone(),
        observation.clone(),
    )));
    callback_entered(&guard).await;
    assert_eq!(
        observation.disposition(),
        FactAccountingDisposition::Unproved
    );
    assert_eq!(
        tuple(&observation.observed_usage().unwrap()),
        tuple(&usage())
    );
    assert_eq!(guard.records.lock().unwrap().len(), 1);
    assert!(
        !actual.is_finished(),
        "the actual callback is still blocked"
    );
    attempt.cancel();
    receiver.close();
    assert!(
        !actual.is_finished(),
        "cancellation cannot finish the actual callback"
    );
    guard.release.add_permits(1);
    let outcome = tokio::time::timeout(Duration::from_secs(5), actual)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.accounting,
        FactAccountingDisposition::CallbackReturned
    );
    assert_eq!(
        observation.disposition(),
        FactAccountingDisposition::CallbackReturned
    );
    assert_eq!(
        guard
            .records
            .lock()
            .unwrap()
            .iter()
            .map(tuple)
            .collect::<Vec<_>>(),
        vec![tuple(&usage())]
    );
    assert_eq!(client.streaming.load(Ordering::SeqCst), 1);
    assert_eq!(client.complete.load(Ordering::SeqCst), 0);
    // The original provider sender was never dropped to manufacture EOF.
    drop(provider_sender);
}

#[tokio::test]
async fn actual_full64_pipe_does_not_defer_already_observed_done_accounting() {
    let guard = Guard::new(vec![], true);
    let (agent, client, provider_sender) = fixture(guard.clone(), false, false);
    for index in 0..61 {
        provider_sender
            .send(StreamEvent::TextDelta(index.to_string()))
            .await
            .unwrap();
    }
    provider_sender
        .send(StreamEvent::Done(response()))
        .await
        .unwrap();
    let observation = Arc::new(FactModelAccountingObservation::default());
    let (frames, receiver) = mpsc::channel(64);
    let capacity = frames.clone();
    let actual = tokio::spawn(send_static(prepared(&agent, client).run(
        frames,
        CancellationToken::new(),
        observation.clone(),
    )));
    callback_entered(&guard).await;
    assert_eq!(
        capacity.capacity(),
        0,
        "three actual evidence frames plus61 actual deltas fill the real pipe"
    );
    assert_eq!(
        guard
            .records
            .lock()
            .unwrap()
            .iter()
            .map(tuple)
            .collect::<Vec<_>>(),
        vec![tuple(&usage())]
    );
    drop(receiver);
    guard.release.add_permits(1);
    let outcome = tokio::time::timeout(Duration::from_secs(5), actual)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.accounting,
        FactAccountingDisposition::CallbackReturned
    );
    assert_eq!(
        observation.disposition(),
        FactAccountingDisposition::CallbackReturned
    );
    drop(provider_sender);
}

#[tokio::test]
async fn fallback_uses_distinct_actual_sequence_and_same_policy_deny_or_allow() {
    for deny in [true, false] {
        let guard = Guard::new(
            vec![
                BudgetDecision::Allow,
                if deny {
                    BudgetDecision::Deny {
                        resource: "tokens".into(),
                        reason: "actual fallback denied".into(),
                    }
                } else {
                    BudgetDecision::Allow
                },
            ],
            false,
        );
        let (agent, client, _provider_sender) = fixture(guard.clone(), true, false);
        let observation = Arc::new(FactModelAccountingObservation::default());
        let (frames, mut receiver) = mpsc::channel(64);
        let actual = tokio::spawn(send_static(prepared(&agent, client.clone()).run(
            frames,
            CancellationToken::new(),
            observation.clone(),
        )));
        let returned = actual.await.unwrap();
        assert_eq!(returned.is_err(), deny);
        assert_eq!(client.streaming.load(Ordering::SeqCst), 1);
        assert_eq!(client.complete.load(Ordering::SeqCst), u64::from(!deny));
        {
            let estimates = guard.estimates.lock().unwrap();
            assert_eq!(estimates.len(), 2);
            assert!(estimates.iter().all(|value| *value > 0));
            assert_eq!(estimates[0], estimates[1]);
        }
        let mut inputs = vec![];
        let mut usages = vec![];
        let mut threshold = false;
        while let Some(frame) = receiver.recv().await {
            match frame {
                FactModelFrame::Input(input) => {
                    input.validate().unwrap();
                    inputs.push(input);
                }
                FactModelFrame::Usage(usage) => {
                    usage.validate().unwrap();
                    usages.push(usage);
                }
                FactModelFrame::Threshold(BudgetDecision::Deny { reason, .. }) => {
                    assert_eq!(reason, "actual fallback denied");
                    threshold = true;
                }
                _ => {}
            }
        }
        assert_eq!(inputs.len(), if deny { 1 } else { 2 });
        assert_eq!(inputs[0].call_sequence, 1);
        if deny {
            assert!(threshold);
            assert!(guard.records.lock().unwrap().is_empty());
            assert_eq!(
                observation.disposition(),
                FactAccountingDisposition::NoSuccessObserved
            );
        } else {
            assert_eq!(inputs[1].call_sequence, 2);
            assert_ne!(inputs[0].snapshot_digest, inputs[1].snapshot_digest);
            assert_eq!(usages.len(), 1);
            usages[0].validate_against(&inputs[1]).unwrap();
            assert_eq!(
                guard
                    .records
                    .lock()
                    .unwrap()
                    .iter()
                    .map(tuple)
                    .collect::<Vec<_>>(),
                vec![tuple(&usage())]
            );
        }
    }
}

struct CachedManaged {
    binding: Mutex<Option<FactModelBinding>>,
}
#[async_trait]
impl LlmClient for CachedManaged {
    fn model_generation_is_managed(&self) -> bool {
        true
    }
    fn bind_fact_model_governance(&self, _: &FactModelBindingRequest) -> Option<FactModelBinding> {
        self.binding.lock().unwrap().take()
    }
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("foreign cached binding must not reach provider");
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        panic!("foreign cached binding must not reach provider");
    }
}

#[tokio::test]
async fn consuming_actual_framework_binding_accepts_same_original_and_refuses_cached_foreign() {
    let guard = Guard::new(vec![], false);
    let (agent, client, provider_sender) = fixture(guard.clone(), false, false);
    let invocation = agent.bound_invocation.as_ref().unwrap();
    let managed = agent.scoped_llm_client(invocation);
    // The real production request constructor and actual facade mint this
    // binding. No private-field literal/test-only authority factory is used.
    let request = original_request(&agent, "fact-original", managed.clone()).unwrap();
    let genuine = managed.bind_fact_model_governance(&request).unwrap();
    genuine.validate_current(&request).unwrap();
    let cached: Arc<dyn LlmClient> = Arc::new(CachedManaged {
        binding: Mutex::new(Some(genuine)),
    });
    assert!(agent
        .prepare_fact_model_call(
            "fact-original",
            cached,
            vec![Message::user("foreign cached request")],
            None,
            vec![]
        )
        .is_err());
    assert!(guard.estimates.lock().unwrap().is_empty());
    assert!(guard.records.lock().unwrap().is_empty());
    assert_eq!(client.streaming.load(Ordering::SeqCst), 0);
    provider_sender
        .send(StreamEvent::Done(response()))
        .await
        .unwrap();
    let (sender, mut receiver) = mpsc::channel(64);
    let observation = Arc::new(FactModelAccountingObservation::default());
    let actual = tokio::spawn(send_static(prepared(&agent, managed).run(
        sender,
        CancellationToken::new(),
        observation,
    )));
    actual.await.unwrap().unwrap();
    let mut done = 0;
    while let Some(frame) = receiver.recv().await {
        if matches!(frame, FactModelFrame::RawStream(StreamEvent::Done(_))) {
            done += 1;
        }
    }
    assert_eq!(done, 1);
    assert_eq!(client.streaming.load(Ordering::SeqCst), 1);
    assert_eq!(guard.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn nonretryable_setup_keeps_actual_error_and_has_no_fallback_or_success_record() {
    let guard = Guard::new(vec![], false);
    let (agent, client, _provider_sender) = fixture(guard.clone(), false, true);
    let (sender, _receiver) = mpsc::channel(64);
    let accounting = Arc::new(FactModelAccountingObservation::default());
    let result = send_static(prepared(&agent, client.clone()).run(
        sender,
        CancellationToken::new(),
        accounting.clone(),
    ))
    .await;
    assert!(
        matches!(result, Err(ActorError::Defect(message)) if message == "terminal controlled provider")
    );
    assert_eq!(client.complete.load(Ordering::SeqCst), 0);
    assert!(guard.records.lock().unwrap().is_empty());
    assert_eq!(
        accounting.disposition(),
        FactAccountingDisposition::NoSuccessObserved
    );
}

#[test]
fn accounting_closed_set_round_trips_and_refuses_unknown() {
    for value in [
        FactAccountingDisposition::NoSuccessObserved,
        FactAccountingDisposition::CallbackReturned,
        FactAccountingDisposition::NoGuard,
        FactAccountingDisposition::Unproved,
    ] {
        assert_eq!(
            FactAccountingDisposition::parse(value.as_str()).unwrap(),
            value
        );
    }
    assert!(FactAccountingDisposition::parse("unknown")
        .unwrap_err()
        .to_string()
        .contains("FactAccountingDisposition"));
}

struct EmptyEmbedding;
#[async_trait]
impl crate::embedding::EmbeddingProvider for EmptyEmbedding {
    fn descriptor(&self) -> crate::embedding::EmbeddingProviderDescriptor {
        crate::embedding::EmbeddingProviderDescriptor::new("controlled-empty", "passive-status", 2)
    }
    async fn embed(
        &self,
        _: crate::embedding::EmbeddingBatchRequest,
        _: CancellationToken,
    ) -> std::result::Result<
        crate::embedding::EmbeddingBatchResponse,
        crate::embedding::EmbeddingProviderError,
    > {
        panic!("empty real catalog must not call an embedding provider");
    }
}

#[tokio::test]
async fn same_source_frozen_environment_survives_actual_status_close_without_live_strong_graph() {
    use crate::workspace::{
        ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceRetrievalOptions,
        WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime, WorkspaceServices,
    };
    let root = tempfile::tempdir().unwrap();
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let runtime = WorkspaceRetrievalRuntime::start(
        catalog,
        WorkspaceRetrievalOptions::new(Arc::new(EmptyEmbedding)),
        CancellationToken::new(),
    )
    .unwrap();
    let services = WorkspaceServices::local(root.path())
        .with_workspace_retrieval(runtime.clone())
        .unwrap();
    let live =
        crate::harness_evidence::RunCapabilityEvidenceSource::from_agent_with_permission_checker(
            &crate::agent::AgentConfig::default(),
            services.clone(),
            None,
            false,
        );
    let services_count = Arc::strong_count(&services);
    let runtime_count = Arc::strong_count(&runtime);
    let frozen = live.freeze_for_fact_attempt();
    assert_eq!(Arc::strong_count(&services), services_count);
    assert_eq!(Arc::strong_count(&runtime), runtime_count);
    let messages = [Message::user("actual frozen observation")];
    let capture = |source: &crate::harness_evidence::RunCapabilityEvidenceSource| {
        let (capability, presentation, input, _) = source
            .capture_with_presentation(
                1,
                ModelCallObservation::with_presentation_application(
                    ModelInputKindV1::Completion,
                    &messages,
                    None,
                    &[],
                    None,
                    1,
                    ModelPresentationApplicationV1::Auxiliary,
                ),
            )
            .unwrap();
        input.validate_against(&capability).unwrap();
        presentation.validate_against(&input).unwrap();
        capability
    };
    let original_frozen = capture(&frozen);
    assert_ne!(
        original_frozen.retrieval.phase,
        WorkspaceRetrievalPhase::Closed
    );
    runtime.close().await;
    let live_closed = capture(&live);
    assert_eq!(live_closed.retrieval.phase, WorkspaceRetrievalPhase::Closed);
    assert_ne!(live_closed.snapshot_digest, original_frozen.snapshot_digest);
    assert_eq!(
        capture(&frozen).snapshot_digest,
        original_frozen.snapshot_digest
    );
    let weak_services = Arc::downgrade(&services);
    let weak_runtime = Arc::downgrade(&runtime);
    drop(live);
    drop(services);
    drop(runtime);
    assert!(weak_services.upgrade().is_none());
    assert!(weak_runtime.upgrade().is_none());
    assert_eq!(
        capture(&frozen).snapshot_digest,
        original_frozen.snapshot_digest
    );
}

#[tokio::test]
async fn compatibility_loop_new_gate_refuses_managed_binding_but_raw_preparation_stays_available() {
    let guard = Guard::new(vec![], false);
    let (mut agent, client, _provider_sender) = fixture(guard.clone(), false, false);
    agent.shared_model_generation_admission = false;
    let managed = agent.scoped_llm_client(agent.bound_invocation.as_ref().unwrap());
    assert!(agent
        .prepare_fact_model_call(
            "fact-original",
            managed,
            vec![Message::user("default separate facade gate")],
            None,
            vec![]
        )
        .is_err());
    assert!(agent
        .prepare_fact_model_call(
            "fact-original",
            client.clone(),
            vec![Message::user("raw current gate")],
            None,
            vec![]
        )
        .is_ok());
    assert!(guard.estimates.lock().unwrap().is_empty());
    assert!(guard.records.lock().unwrap().is_empty());
    assert_eq!(client.streaming.load(Ordering::SeqCst), 0);
}
