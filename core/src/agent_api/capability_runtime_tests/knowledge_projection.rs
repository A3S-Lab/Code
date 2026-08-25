use super::*;

use crate::cognitive_context::{
    CognitiveContextDocumentV1, CognitiveContextLimits, CognitiveContextProvider,
    CognitiveContextRequestV1, CognitiveContextResponseV1, CognitiveContextResult,
    CognitiveContextSession, CognitiveKnowledgeBindingV1, CognitiveKnowledgeCitationV1,
    CognitivePackageBindingV1,
};
use crate::store::SessionStore;
use sha2::{Digest, Sha256};

const CANONICAL_DIGEST_PREFIX: &[u8] = b"agentic-ontology-canonical-v1\0";
const CAPABILITY_SNAPSHOT_DIGEST_DOMAIN: &str = "a3s.use.capability-snapshot.v1";

struct VersionedKnowledgeProvider {
    name: String,
    marker: &'static str,
    requests: Mutex<Vec<CognitiveContextRequestV1>>,
}

#[async_trait]
impl CognitiveContextProvider for VersionedKnowledgeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn query(
        &self,
        request: &CognitiveContextRequestV1,
    ) -> CognitiveContextResult<CognitiveContextResponseV1> {
        self.requests.lock().unwrap().push(request.clone());
        let citation = CognitiveKnowledgeCitationV1::new(
            &request.binding,
            "concepts/projected-knowledge.md",
            "Projected knowledge",
            vec![format!("sha256:{}", "e".repeat(64))],
        )?;
        let document = CognitiveContextDocumentV1::new(citation, self.marker)?;
        CognitiveContextResponseV1::new(request, vec![document], false)
    }
}

struct KnowledgeCutoverClient {
    calls: AtomicUsize,
    observed: Mutex<Vec<&'static str>>,
    old_generation_observed: Semaphore,
    release_old_call: Semaphore,
}

struct CheckpointKnowledgeCutoverClient {
    calls: AtomicUsize,
    old_generation_observed: Semaphore,
    release_old_call: Semaphore,
}

impl CheckpointKnowledgeCutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            old_generation_observed: Semaphore::new(0),
            release_old_call: Semaphore::new(0),
        }
    }

    fn observe_old_generation(&self, system: Option<&str>) -> anyhow::Result<()> {
        let system = system.ok_or_else(|| anyhow::anyhow!("system prompt is missing"))?;
        anyhow::ensure!(system.contains("PROJECTED_KNOWLEDGE_GENERATION_ONE"));
        anyhow::ensure!(!system.contains("PROJECTED_KNOWLEDGE_GENERATION_TWO"));
        Ok(())
    }
}

#[async_trait]
impl LlmClient for CheckpointKnowledgeCutoverClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.observe_old_generation(system)?;
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.old_generation_observed.add_permits(1);
                self.release_old_call.acquire().await?.forget();
                Ok(LlmResponse {
                    message: Message {
                        role: "assistant".to_owned(),
                        content: vec![ContentBlock::ToolUse {
                            id: "checkpoint-search".to_owned(),
                            name: "search_skills".to_owned(),
                            input: serde_json::json!({"query": "checkpoint"}),
                        }],
                        reasoning_content: None,
                    },
                    usage: TokenUsage {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        total_tokens: 2,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                    },
                    stop_reason: Some("tool_use".to_owned()),
                    token_logprobs: Vec::new(),
                    meta: None,
                })
            }
            1 => Ok(CutoverClient::final_text("checkpoint complete")),
            call => anyhow::bail!("unexpected CheckpointKnowledgeCutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the checkpoint cutover test")
    }
}

#[derive(Default)]
struct RecordingCheckpointExportSink {
    exports: Mutex<Vec<crate::SessionCheckpointExportV1>>,
}

#[async_trait]
impl crate::SessionCheckpointExportSink for RecordingCheckpointExportSink {
    async fn export_checkpoint(
        &self,
        checkpoint: crate::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        self.exports.lock().unwrap().push(checkpoint);
        Ok(())
    }
}

impl KnowledgeCutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed: Mutex::new(Vec::new()),
            old_generation_observed: Semaphore::new(0),
            release_old_call: Semaphore::new(0),
        }
    }

    fn observe(&self, system: Option<&str>) -> anyhow::Result<&'static str> {
        let system = system.ok_or_else(|| anyhow::anyhow!("system prompt is missing"))?;
        let generation_one = system.contains("PROJECTED_KNOWLEDGE_GENERATION_ONE");
        let generation_two = system.contains("PROJECTED_KNOWLEDGE_GENERATION_TWO");
        let observed = match (generation_one, generation_two) {
            (true, false) => "generation-one",
            (false, true) => "generation-two",
            _ => anyhow::bail!(
                "expected exactly one projected Knowledge generation in the system prompt"
            ),
        };
        self.observed.lock().unwrap().push(observed);
        Ok(observed)
    }
}

#[async_trait]
impl LlmClient for KnowledgeCutoverClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let observed = self.observe(system)?;
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                anyhow::ensure!(observed == "generation-one");
                self.old_generation_observed.add_permits(1);
                self.release_old_call.acquire().await?.forget();
                Ok(CutoverClient::final_text(
                    "knowledge generation one complete",
                ))
            }
            1 => {
                anyhow::ensure!(observed == "generation-two");
                Ok(CutoverClient::final_text(
                    "knowledge generation two complete",
                ))
            }
            call => anyhow::bail!("unexpected KnowledgeCutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the Knowledge cutover test")
    }
}

fn capability_snapshot_digest(
    package_id: &str,
    package_version: &str,
    lifecycle_generation: u64,
    generation_digest: &str,
    knowledge: &CognitiveKnowledgeBindingV1,
) -> String {
    let encoded = serde_json::to_vec(&(
        package_id,
        package_version,
        lifecycle_generation,
        generation_digest,
        knowledge.surface_id.as_str(),
        knowledge.format_version.as_str(),
        knowledge.content_digest.as_str(),
    ))
    .unwrap();
    let mut hasher = Sha256::new();
    hasher.update(CANONICAL_DIGEST_PREFIX);
    hasher.update((CAPABILITY_SNAPSHOT_DIGEST_DOMAIN.len() as u64).to_be_bytes());
    hasher.update(CAPABILITY_SNAPSHOT_DIGEST_DOMAIN.as_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    format!("sha256:{:x}", hasher.finalize())
}

fn knowledge_binding(generation: u64, digest_byte: char) -> CognitivePackageBindingV1 {
    let package_id = "acme/projected-knowledge";
    let package_version = format!("1.0.{generation}");
    let generation_digest = format!("sha256:{}", digest_byte.to_string().repeat(64));
    let knowledge = CognitiveKnowledgeBindingV1::new(
        "projected-knowledge",
        "0.2",
        format!(
            "sha256:{}",
            ((digest_byte as u8 + 1) as char).to_string().repeat(64)
        ),
        generation,
        generation_digest.clone(),
    )
    .unwrap();
    let snapshot_digest = capability_snapshot_digest(
        package_id,
        &package_version,
        generation,
        &generation_digest,
        &knowledge,
    );
    CognitivePackageBindingV1::new(
        package_id,
        package_version,
        generation,
        generation_digest,
        snapshot_digest,
        knowledge,
        CognitiveContextLimits::default(),
    )
    .unwrap()
}

fn projected_knowledge(
    name: &str,
    binding: CognitivePackageBindingV1,
    marker: &'static str,
) -> Arc<CognitiveContextSession> {
    let provider: Arc<dyn CognitiveContextProvider> = Arc::new(VersionedKnowledgeProvider {
        name: name.to_owned(),
        marker,
        requests: Mutex::new(Vec::new()),
    });
    Arc::new(CognitiveContextSession::new(binding, provider).unwrap())
}

fn knowledge_batch(
    code_generation: u64,
    use_revision: char,
    descriptor_digest: char,
    knowledge: Arc<CognitiveContextSession>,
    acquired: &Arc<AtomicUsize>,
    dropped: &Arc<AtomicUsize>,
) -> SessionCapabilityBatch {
    let upstream = use_generation(code_generation, use_revision);
    let (set, ids) = use_kind_set(
        code_generation,
        upstream.clone(),
        CapabilityKind::Knowledge,
        &[(knowledge.provider_name(), descriptor_digest)],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, acquired, dropped))
            .unwrap();
    batch
        .stage_value(
            ids[knowledge.provider_name()].clone(),
            CapabilityValue::Knowledge(knowledge),
        )
        .unwrap();
    batch
}

#[tokio::test]
async fn projected_knowledge_is_run_frozen_and_records_each_exact_binding() {
    let client = Arc::new(KnowledgeCutoverClient::new());
    let session =
        Arc::new(test_session_with_client("projected-knowledge-cutover", client.clone()).await);
    let first_binding = knowledge_binding(1, 'a');
    let second_binding = knowledge_binding(2, 'c');
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    session
        .apply_capability_batch(
            knowledge_batch(
                1,
                'a',
                'b',
                projected_knowledge(
                    "projected-knowledge",
                    first_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_ONE",
                ),
                &first_acquired,
                &first_dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Use exact projected Knowledge N.", None).await }
    });
    client
        .old_generation_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    session
        .apply_capability_batch(
            knowledge_batch(
                2,
                'c',
                'd',
                projected_knowledge(
                    "projected-knowledge",
                    second_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_TWO",
                ),
                &second_acquired,
                &second_dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    client.release_old_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "knowledge generation one complete");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_result = session
        .send("Use exact projected Knowledge N+1.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "knowledge generation two complete");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed.lock().unwrap(),
        &["generation-one", "generation-two"]
    );

    let mut runs = session.runs().await;
    runs.sort_by_key(|run| run.created_at_ms);
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0].cognitive_package_binding.as_ref(),
        Some(&first_binding)
    );
    assert_eq!(
        runs[1].cognitive_package_binding.as_ref(),
        Some(&second_binding)
    );
    assert_eq!(
        session.current_cognitive_package_binding(),
        Some(second_binding)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_checkpoint_keeps_source_run_knowledge_across_concurrent_cutover() {
    let client = Arc::new(CheckpointKnowledgeCutoverClient::new());
    let sink = Arc::new(RecordingCheckpointExportSink::default());
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/projected-knowledge-checkpoint".to_owned(),
                Arc::clone(&client) as Arc<dyn LlmClient>,
                &SessionOptions::new()
                    .with_session_id("projected-knowledge-checkpoint")
                    .with_session_checkpoint_export_sink(sink.clone())
                    .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_continuation(false),
            )
            .unwrap(),
    );
    let first_binding = knowledge_binding(1, 'a');
    let second_binding = knowledge_binding(2, 'c');
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    session
        .apply_capability_batch(
            knowledge_batch(
                1,
                'a',
                'b',
                projected_knowledge(
                    "projected-knowledge",
                    first_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_ONE",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Export the N boundary.", None).await }
    });
    client
        .old_generation_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    session
        .apply_capability_batch(
            knowledge_batch(
                2,
                'c',
                'd',
                projected_knowledge(
                    "projected-knowledge",
                    second_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_TWO",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    client.release_old_call.add_permits(1);
    assert_eq!(old_run.await.unwrap().unwrap().text, "checkpoint complete");
    assert_eq!(
        session.current_cognitive_package_binding(),
        Some(second_binding)
    );

    let export = sink
        .exports
        .lock()
        .unwrap()
        .pop()
        .expect("one completed tool round must be exported");
    let payload = export.open().unwrap();
    let logical = payload.logical_resume.unwrap();
    let source = payload
        .snapshot
        .run_records
        .iter()
        .find(|record| record.snapshot.id == logical.run_id)
        .unwrap();
    assert_eq!(
        source.snapshot.cognitive_package_binding.as_ref(),
        Some(&first_binding)
    );
    assert_eq!(
        payload.snapshot.session.cognitive_package_binding.as_ref(),
        Some(&first_binding),
        "a portable live checkpoint must resume under the source Run authority"
    );
    session.close().await;
}

#[tokio::test]
async fn projected_knowledge_rejects_multiple_authorities() {
    let session = test_session("projected-knowledge-conflicts").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Knowledge,
        &[("knowledge-one", 'b'), ("knowledge-two", 'c')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["knowledge-one"].clone(),
            CapabilityValue::Knowledge(projected_knowledge(
                "knowledge-one",
                knowledge_binding(1, 'a'),
                "ONE",
            )),
        )
        .unwrap();
    batch
        .stage_value(
            ids["knowledge-two"].clone(),
            CapabilityValue::Knowledge(projected_knowledge(
                "knowledge-two",
                knowledge_binding(1, 'a'),
                "TWO",
            )),
        )
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("one Run cannot carry multiple cognitive authorities");

    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Knowledge,
            ref message,
            ..
        } if message.contains("exactly one")
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn projected_knowledge_rejects_session_static_general_context() {
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let ambient: Arc<dyn crate::context::ContextProvider> = Arc::new(
        crate::context::StaticContextProvider::new("ambient-context"),
    );
    let session = agent
        .build_session(
            "/tmp/projected-knowledge-ambient-context".to_string(),
            Arc::new(NoopClient),
            &SessionOptions::new()
                .with_session_id("projected-knowledge-ambient-context")
                .with_context_provider(ambient),
        )
        .unwrap();
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(
            knowledge_batch(
                1,
                'a',
                'b',
                projected_knowledge(
                    "projected-knowledge",
                    knowledge_binding(1, 'a'),
                    "SHOULD_NOT_BE_PUBLISHED",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .expect_err("exact Knowledge must not compose with ambient host Context");

    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Knowledge,
            ref message,
            ..
        } if message.contains("Session-static general-purpose Context")
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn projected_general_context_rejects_session_static_knowledge() {
    let static_provider: Arc<dyn CognitiveContextProvider> = Arc::new(VersionedKnowledgeProvider {
        name: "static-knowledge".to_owned(),
        marker: "STATIC_KNOWLEDGE",
        requests: Mutex::new(Vec::new()),
    });
    let static_context =
        CognitiveContextSession::new(knowledge_binding(1, 'a'), static_provider).unwrap();
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .build_session(
            "/tmp/projected-context-static-knowledge".to_string(),
            Arc::new(NoopClient),
            &SessionOptions::new()
                .with_session_id("projected-context-static-knowledge")
                .with_cognitive_context(static_context),
        )
        .unwrap();
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Context,
        &[("ambient-context", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    let ambient: Arc<dyn crate::context::ContextProvider> = Arc::new(
        crate::context::StaticContextProvider::new("ambient-context"),
    );
    batch
        .stage_value(
            ids["ambient-context"].clone(),
            CapabilityValue::Context(ambient),
        )
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("ambient projected Context must not compose with exact Knowledge");

    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Context,
            ref message,
            ..
        } if message.contains("exact Session cognitive binding")
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn session_static_knowledge_requires_an_exact_projection_bootstrap() {
    let static_binding = knowledge_binding(1, 'a');
    let static_provider: Arc<dyn CognitiveContextProvider> = Arc::new(VersionedKnowledgeProvider {
        name: "projected-knowledge".to_owned(),
        marker: "STATIC_KNOWLEDGE",
        requests: Mutex::new(Vec::new()),
    });
    let static_context =
        CognitiveContextSession::new(static_binding.clone(), static_provider).unwrap();
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .build_session(
            "/tmp/projected-knowledge-bootstrap".to_string(),
            Arc::new(NoopClient),
            &SessionOptions::new()
                .with_session_id("projected-knowledge-bootstrap")
                .with_cognitive_context(static_context),
        )
        .unwrap();
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let before = session.capability_catalog_stamp();

    let drift = knowledge_batch(
        1,
        'c',
        'd',
        projected_knowledge(
            "projected-knowledge",
            knowledge_binding(2, 'c'),
            "PROJECTED_KNOWLEDGE_GENERATION_TWO",
        ),
        &acquired,
        &dropped,
    );
    let error = session
        .apply_capability_batch(drift, CancellationToken::new())
        .await
        .expect_err("a static recovery seed must first be projected exactly");
    assert!(
        matches!(
            error,
            CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Knowledge,
                ref message,
                ..
            } if message.contains("bootstrap")
        ),
        "unexpected bootstrap error: {error:?}"
    );
    assert_eq!(session.capability_catalog_stamp(), before);

    session
        .apply_capability_batch(
            knowledge_batch(
                1,
                'a',
                'b',
                projected_knowledge(
                    "projected-knowledge",
                    static_binding,
                    "PROJECTED_KNOWLEDGE_GENERATION_ONE",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    session
        .apply_capability_batch(
            knowledge_batch(
                2,
                'c',
                'd',
                projected_knowledge(
                    "projected-knowledge",
                    knowledge_binding(2, 'c'),
                    "PROJECTED_KNOWLEDGE_GENERATION_TWO",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let error = session
        .apply_capability_batch(
            SessionCapabilityBatch::new(
                CapabilitySet::from_contributions(
                    CodeCatalogGeneration::new(3),
                    Vec::<CapabilityContribution>::new(),
                )
                .unwrap(),
            )
            .unwrap(),
            CancellationToken::new(),
        )
        .await
        .expect_err("removal must not reveal the stale static recovery seed");
    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Knowledge,
            ref message,
            ..
        } if message.contains("stale")
    ));
}

#[tokio::test]
async fn projected_knowledge_persists_latest_while_old_runs_keep_their_binding() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let client = Arc::new(KnowledgeCutoverClient::new());
    let agent = Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let session = Arc::new(
        agent
            .session_async(
                "/tmp/projected-knowledge-persistence",
                Some(
                    SessionOptions::new()
                        .with_session_id("projected-knowledge-persistence")
                        .with_session_store(store.clone())
                        .with_llm_client(client.clone())
                        .with_permission_policy(
                            crate::permissions::PermissionPolicy::new().allow("*"),
                        )
                        .with_planning_mode(crate::prompts::PlanningMode::Disabled),
                ),
            )
            .await
            .unwrap(),
    );
    let first_binding = knowledge_binding(1, 'a');
    let second_binding = knowledge_binding(2, 'c');
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    session
        .apply_capability_batch(
            knowledge_batch(
                1,
                'a',
                'b',
                projected_knowledge(
                    "projected-knowledge",
                    first_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_ONE",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Persist Knowledge N.", None).await }
    });
    client
        .old_generation_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    session
        .apply_capability_batch(
            knowledge_batch(
                2,
                'c',
                'd',
                projected_knowledge(
                    "projected-knowledge",
                    second_binding.clone(),
                    "PROJECTED_KNOWLEDGE_GENERATION_TWO",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    client.release_old_call.add_permits(1);
    old_run.await.unwrap().unwrap();
    session.send("Persist Knowledge N+1.", None).await.unwrap();
    session.save().await.unwrap();

    let snapshot = store
        .load_snapshot("projected-knowledge-persistence")
        .await
        .unwrap()
        .unwrap();
    snapshot.validate_invariants().unwrap();
    let mut tampered = snapshot.clone();
    let event_binding = tampered
        .run_records
        .iter_mut()
        .flat_map(|record| record.events.iter_mut())
        .find_map(|event| match &mut event.event {
            crate::agent::AgentEvent::CognitiveContextBound { binding } => Some(binding),
            _ => None,
        })
        .expect("cognitive Run must retain binding evidence");
    event_binding.limits.max_results -= 1;
    event_binding.validate().unwrap();
    assert!(tampered.validate_invariants().is_err());
    assert_eq!(
        snapshot.session.cognitive_package_binding.as_ref(),
        Some(&second_binding)
    );
    let mut run_bindings = snapshot
        .run_records
        .iter()
        .map(|record| record.snapshot.cognitive_package_binding.clone())
        .collect::<Vec<_>>();
    run_bindings.sort_by_key(|binding| {
        binding
            .as_ref()
            .map_or(0, |binding| binding.lifecycle_generation)
    });
    assert_eq!(
        run_bindings,
        vec![Some(first_binding), Some(second_binding)]
    );

    session.close().await;
    drop(session);
    let resume_provider: Arc<dyn CognitiveContextProvider> = Arc::new(VersionedKnowledgeProvider {
        name: "projected-knowledge".to_owned(),
        marker: "RESUMED_PROJECTED_KNOWLEDGE",
        requests: Mutex::new(Vec::new()),
    });
    let resume_context =
        CognitiveContextSession::new(knowledge_binding(2, 'c'), resume_provider).unwrap();
    let resumed = agent
        .resume_session_async(
            "projected-knowledge-persistence",
            SessionOptions::new()
                .with_session_store(store)
                .with_llm_client(Arc::new(NoopClient))
                .with_cognitive_context(resume_context),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed.current_cognitive_package_binding(),
        Some(knowledge_binding(2, 'c'))
    );
    resumed
        .apply_capability_batch(
            knowledge_batch(
                1,
                'e',
                'f',
                projected_knowledge(
                    "projected-knowledge",
                    knowledge_binding(2, 'c'),
                    "RESUMED_PROJECTED_KNOWLEDGE",
                ),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .expect("resume seed must bootstrap the catalog with the persisted exact binding");
}
