use super::*;

use crate::context::{
    ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType, StaticContextProvider,
};

struct ContextCutoverClient {
    calls: AtomicUsize,
    observed_context: Mutex<Vec<&'static str>>,
    old_context_observed: Semaphore,
    release_old_call: Semaphore,
}

impl ContextCutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed_context: Mutex::new(Vec::new()),
            old_context_observed: Semaphore::new(0),
            release_old_call: Semaphore::new(0),
        }
    }

    fn observe_context(&self, system: Option<&str>) -> anyhow::Result<&'static str> {
        let system = system.ok_or_else(|| anyhow::anyhow!("system prompt is missing"))?;
        let generation_one = system.contains("PROJECTED_CONTEXT_GENERATION_ONE");
        let generation_two = system.contains("PROJECTED_CONTEXT_GENERATION_TWO");
        let observed = match (generation_one, generation_two) {
            (true, false) => "generation-one",
            (false, true) => "generation-two",
            _ => anyhow::bail!(
                "expected exactly one projected Context generation in the system prompt"
            ),
        };
        self.observed_context.lock().unwrap().push(observed);
        Ok(observed)
    }
}

#[async_trait]
impl LlmClient for ContextCutoverClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let observed = self.observe_context(system)?;
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                anyhow::ensure!(observed == "generation-one");
                self.old_context_observed.add_permits(1);
                self.release_old_call.acquire().await?.forget();
                Ok(CutoverClient::final_text("context generation one complete"))
            }
            1 => {
                anyhow::ensure!(observed == "generation-two");
                Ok(CutoverClient::final_text("context generation two complete"))
            }
            call => anyhow::bail!("unexpected ContextCutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the Context cutover test")
    }
}

fn projected_context(name: &str, marker: &str) -> Arc<dyn ContextProvider> {
    Arc::new(
        StaticContextProvider::new(name).with_item(
            ContextItem::new(
                format!("{name}-item"),
                ContextType::Resource,
                marker.to_string(),
            )
            .with_required(),
        ),
    )
}

struct BoundContextProvider {
    binding: crate::cognitive_context::CognitivePackageBindingV1,
}

#[async_trait]
impl ContextProvider for BoundContextProvider {
    fn name(&self) -> &str {
        "projected-bound-context"
    }

    fn cognitive_package_binding(
        &self,
    ) -> Option<&crate::cognitive_context::CognitivePackageBindingV1> {
        Some(&self.binding)
    }

    async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
        Ok(ContextResult::new(self.name()))
    }
}

fn cognitive_binding() -> crate::cognitive_context::CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = crate::cognitive_context::CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    crate::cognitive_context::CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        crate::cognitive_context::CognitiveContextLimits::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn projected_context_is_run_frozen_across_atomic_cutover() {
    let client = Arc::new(ContextCutoverClient::new());
    let session =
        Arc::new(test_session_with_client("projected-context-cutover", client.clone()).await);
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_kind_set(
        1,
        first_upstream.clone(),
        CapabilityKind::Context,
        &[("projected-context", 'b')],
    );
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected-context"].clone(),
            CapabilityValue::Context(projected_context(
                "projected-context",
                "PROJECTED_CONTEXT_GENERATION_ONE",
            )),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Use the exact projected context.", None).await }
    });
    client
        .old_context_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_kind_set(
        2,
        second_upstream.clone(),
        CapabilityKind::Context,
        &[("projected-context", 'd')],
    );
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected-context"].clone(),
            CapabilityValue::Context(projected_context(
                "projected-context",
                "PROJECTED_CONTEXT_GENERATION_TWO",
            )),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    client.release_old_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "context generation one complete");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_result = session
        .send("Use the new exact projected context.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "context generation two complete");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed_context.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}

#[tokio::test]
async fn projected_context_cannot_shadow_a_session_static_provider() {
    let session = test_session("projected-context-name-conflict").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Context,
        &[("skills_catalog", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["skills_catalog"].clone(),
            CapabilityValue::Context(projected_context(
                "skills_catalog",
                "SHOULD_NOT_BE_PUBLISHED",
            )),
        )
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("projected Context must not shadow a Session provider");

    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Context,
            ref public_name,
        } if public_name == "skills_catalog"
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn projected_context_cannot_bypass_the_persisted_knowledge_binding() {
    let session = test_session("projected-context-cognitive-binding").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Context,
        &[("projected-bound-context", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    let provider: Arc<dyn ContextProvider> = Arc::new(BoundContextProvider {
        binding: cognitive_binding(),
    });
    batch
        .stage_value(
            ids["projected-bound-context"].clone(),
            CapabilityValue::Context(provider),
        )
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("Context projection must not bypass persisted Knowledge authority");

    assert!(matches!(
        error,
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Context,
            ref public_name,
            ref message,
        } if public_name == "projected-bound-context"
            && message.contains("persisted Knowledge/session boundary")
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}
