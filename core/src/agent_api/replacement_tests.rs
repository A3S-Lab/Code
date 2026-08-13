use super::tests::{
    test_cognitive_binding, test_cognitive_context, test_config, StaticStreamingClient,
    TestCognitiveProviderMode,
};
use super::*;
use crate::store::{MemorySessionStore, SessionStore};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

fn replacement_options(store: Arc<MemorySessionStore>, model: &str) -> SessionOptions {
    replacement_options_with_store(store, model)
}

fn replacement_options_with_store(store: Arc<dyn SessionStore>, model: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_store(store)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_model(model)
}

fn capability_snapshot_digest(
    binding: &crate::cognitive_context::CognitivePackageBindingV1,
) -> String {
    let encoded = serde_json::to_vec(&(
        binding.package_id.as_str(),
        binding.package_version.as_str(),
        binding.lifecycle_generation,
        &binding.generation_digest,
        binding.knowledge.surface_id.as_str(),
        binding.knowledge.format_version.as_str(),
        binding.knowledge.content_digest.as_str(),
    ))
    .unwrap();
    let domain = b"a3s.use.capability-snapshot.v1";
    let mut hasher = Sha256::new();
    hasher.update(b"agentic-ontology-canonical-v1\0");
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain);
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    format!("sha256:{:x}", hasher.finalize())
}

fn next_cognitive_context(
    version: &str,
    lifecycle_generation: u64,
    digest_byte: u8,
) -> (
    crate::cognitive_context::CognitiveContextSession,
    crate::cognitive_context::CognitivePackageBindingV1,
) {
    let mut binding = test_cognitive_binding();
    binding.package_version = version.to_string();
    binding.lifecycle_generation = lifecycle_generation;
    binding.generation_digest = format!("sha256:{}", format!("{digest_byte:02x}").repeat(32));
    binding.knowledge.lifecycle_generation = lifecycle_generation;
    binding.knowledge.generation_digest = binding.generation_digest.clone();
    binding.capability_snapshot_digest = capability_snapshot_digest(&binding);
    binding.validate().unwrap();
    let provider: Arc<dyn crate::cognitive_context::CognitiveContextProvider> = Arc::new(
        super::tests::TestCognitiveProvider::new(TestCognitiveProviderMode::Valid),
    );
    let context =
        crate::cognitive_context::CognitiveContextSession::new(binding.clone(), provider).unwrap();
    (context, binding)
}

struct FailNthSnapshotSaveStore {
    inner: MemorySessionStore,
    saves: AtomicUsize,
    fail_on: usize,
}

impl FailNthSnapshotSaveStore {
    fn new(fail_on: usize) -> Self {
        Self {
            inner: MemorySessionStore::new(),
            saves: AtomicUsize::new(0),
            fail_on,
        }
    }
}

#[async_trait::async_trait]
impl SessionStore for FailNthSnapshotSaveStore {
    async fn save(&self, session: &crate::store::SessionData) -> anyhow::Result<()> {
        self.inner.save(session).await
    }

    async fn load(&self, id: &str) -> anyhow::Result<Option<crate::store::SessionData>> {
        self.inner.load(id).await
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.inner.delete(id).await
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        self.inner.list().await
    }

    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        self.inner.exists(id).await
    }

    async fn save_snapshot(
        &self,
        snapshot: &crate::store::SessionSnapshotV1,
    ) -> anyhow::Result<()> {
        let save = self.saves.fetch_add(1, Ordering::SeqCst) + 1;
        if save == self.fail_on {
            anyhow::bail!("injected replacement snapshot failure");
        }
        self.inner.save_snapshot(snapshot).await
    }

    async fn load_snapshot(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<crate::store::SessionSnapshotV1>> {
        self.inner.load_snapshot(id).await
    }

    fn capabilities(&self) -> crate::store::SessionStoreCapabilities {
        self.inner.capabilities()
    }
}

#[tokio::test]
async fn replacement_is_atomic_and_keeps_the_session_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "atomic-session-replacement";
    let current = agent
        .session_async(
            "/tmp/atomic-session-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id),
            ),
        )
        .await
        .unwrap();

    let replacement = agent
        .replace_session_async(
            &current,
            replacement_options(Arc::clone(&store), "openai/gpt-4o"),
        )
        .await
        .unwrap();

    assert!(current.is_closed());
    assert!(!replacement.is_closed());
    assert_eq!(replacement.session_id(), session_id);
    assert_eq!(replacement.model_name, "openai/gpt-4o");
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
}

#[tokio::test]
async fn failed_replacement_leaves_the_current_session_live() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "failed-session-replacement";
    let current = agent
        .session_async(
            "/tmp/failed-session-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id),
            ),
        )
        .await
        .unwrap();

    let error = agent
        .replace_session_async(
            &current,
            SessionOptions::new()
                .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                .with_model("openai/gpt-4o"),
        )
        .await
        .expect_err("replacement without a session store must fail");

    assert!(error.to_string().contains("session_store"));
    assert!(!current.is_closed());
    current.save().await.unwrap();
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
}

#[tokio::test]
async fn bound_replacement_requires_the_host_to_reinject_cognitive_context() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "bound-replacement-requires-context";
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let current = agent
        .session_async(
            "/tmp/bound-replacement-requires-context",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id)
                    .with_cognitive_context(cognitive_context),
            ),
        )
        .await
        .unwrap();

    let error = agent
        .replace_session_async(
            &current,
            replacement_options(Arc::clone(&store), "openai/gpt-4o"),
        )
        .await
        .expect_err("a bound replacement must not silently remove cognition");

    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));
    assert!(!current.is_closed());
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
}

#[tokio::test]
async fn failed_replacement_commit_leaves_current_live_and_persisted_binding_unchanged() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    // First save persists the current snapshot; the second is the replacement
    // commit that must complete before registry cutover.
    let store = Arc::new(FailNthSnapshotSaveStore::new(2));
    let store_port: Arc<dyn SessionStore> = store.clone();
    let session_id = "failed-cognitive-replacement-commit";
    let current = agent
        .session_async(
            "/tmp/failed-cognitive-replacement-commit",
            Some(
                replacement_options_with_store(
                    Arc::clone(&store_port),
                    "anthropic/claude-sonnet-4-20250514",
                )
                .with_session_id(session_id),
            ),
        )
        .await
        .unwrap();
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);

    let error = agent
        .replace_session_async(
            &current,
            replacement_options_with_store(
                Arc::clone(&store_port),
                "anthropic/claude-sonnet-4-20250514",
            )
            .with_cognitive_context(cognitive_context),
        )
        .await
        .expect_err("replacement snapshot failure must abort registry cutover");

    assert!(error
        .to_string()
        .contains("injected replacement snapshot failure"));
    assert!(!current.is_closed());
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
    let snapshot = store
        .load_snapshot(session_id)
        .await
        .unwrap()
        .expect("current snapshot remains durable");
    assert!(snapshot.session.cognitive_package_binding.is_none());
}

#[tokio::test]
async fn replacement_can_bind_an_unbound_conversation_without_rewriting_history() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "cognitive-session-replacement";
    let current = agent
        .session_async(
            "/tmp/cognitive-session-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id)
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unbound answer"))),
            ),
        )
        .await
        .unwrap();
    current
        .send("Remember this conversation", None)
        .await
        .unwrap();
    let history = serde_json::to_value(current.history()).unwrap();
    let (cognitive_context, provider) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let binding = cognitive_context.binding().clone();

    let replacement = agent
        .replace_session_async(
            &current,
            replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                .with_llm_client(Arc::new(StaticStreamingClient::new("grounded answer")))
                .with_cognitive_context(cognitive_context),
        )
        .await
        .unwrap();

    assert!(current.is_closed());
    assert_eq!(
        serde_json::to_value(replacement.history()).unwrap(),
        history
    );
    assert_eq!(replacement.cognitive_package_binding(), Some(&binding));
    replacement
        .send("Use the admitted cognition", None)
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    replacement.save().await.unwrap();

    let snapshot = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(snapshot.session.cognitive_package_binding, Some(binding));
    assert!(snapshot.prior_cognitive_package_bindings.is_empty());
    snapshot.validate_for_session(session_id).unwrap();
}

#[tokio::test]
async fn replacement_advances_the_current_binding_and_retains_prior_run_evidence() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "cognitive-generation-replacement";
    let (first_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let first_binding = first_context.binding().clone();
    let first = agent
        .session_async(
            "/tmp/cognitive-generation-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id)
                    .with_llm_client(Arc::new(StaticStreamingClient::new("first generation")))
                    .with_cognitive_context(first_context),
            ),
        )
        .await
        .unwrap();
    first.send("Use the first generation", None).await.unwrap();

    let mut next_binding = test_cognitive_binding();
    next_binding.package_version = "0.2.0".to_string();
    next_binding.lifecycle_generation = 8;
    next_binding.generation_digest =
        "sha256:bb0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20".to_string();
    next_binding.knowledge.lifecycle_generation = 8;
    next_binding.knowledge.generation_digest = next_binding.generation_digest.clone();
    next_binding.capability_snapshot_digest = capability_snapshot_digest(&next_binding);
    next_binding.validate().unwrap();
    let provider = Arc::new(super::tests::TestCognitiveProvider::new(
        TestCognitiveProviderMode::Valid,
    ));
    let provider_port: Arc<dyn crate::cognitive_context::CognitiveContextProvider> =
        provider.clone();
    let next_context =
        crate::cognitive_context::CognitiveContextSession::new(next_binding.clone(), provider_port)
            .unwrap();

    let replacement = agent
        .replace_session_async(
            &first,
            replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                .with_llm_client(Arc::new(StaticStreamingClient::new("next generation")))
                .with_cognitive_context(next_context),
        )
        .await
        .unwrap();
    assert_eq!(replacement.cognitive_package_binding(), Some(&next_binding));
    replacement
        .send("Use the next generation", None)
        .await
        .unwrap();
    replacement.save().await.unwrap();

    let snapshot = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(
        snapshot.session.cognitive_package_binding,
        Some(next_binding)
    );
    assert_eq!(
        snapshot.prior_cognitive_package_bindings,
        vec![first_binding]
    );
    assert_eq!(
        snapshot
            .run_records
            .iter()
            .flat_map(|record| &record.events)
            .filter(|event| matches!(
                event.event,
                crate::agent::AgentEvent::CognitiveContextBound { .. }
            ))
            .count(),
        2
    );
    snapshot.validate_for_session(session_id).unwrap();
}

#[tokio::test]
async fn repeated_cognitive_replacements_commit_before_return_and_resume_exactly() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "repeated-cognitive-replacement";
    let unbound = agent
        .session_async(
            "/tmp/repeated-cognitive-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id)
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unbound"))),
            ),
        )
        .await
        .unwrap();
    unbound.send("Before cognition", None).await.unwrap();
    let history = serde_json::to_value(unbound.history()).unwrap();

    let (generation_one, binding_one) = next_cognitive_context("0.2.0", 8, 0xbb);
    let generation_one = agent
        .replace_session_async(
            &unbound,
            replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                .with_llm_client(Arc::new(StaticStreamingClient::new("generation one")))
                .with_cognitive_context(generation_one),
        )
        .await
        .unwrap();
    // No caller save is needed to make the new current binding durable.
    let committed_one = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(
        committed_one.session.cognitive_package_binding,
        Some(binding_one.clone())
    );
    assert_eq!(
        serde_json::to_value(generation_one.history()).unwrap(),
        history
    );
    generation_one
        .send("Use generation one", None)
        .await
        .unwrap();

    let (generation_two, binding_two) = next_cognitive_context("0.3.0", 9, 0xcc);
    let generation_two = agent
        .replace_session_async(
            &generation_one,
            replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                .with_llm_client(Arc::new(StaticStreamingClient::new("generation two")))
                .with_cognitive_context(generation_two),
        )
        .await
        .unwrap();
    let committed_two = store.load_snapshot(session_id).await.unwrap().unwrap();
    assert_eq!(
        committed_two.session.cognitive_package_binding,
        Some(binding_two.clone())
    );
    assert_eq!(
        committed_two.prior_cognitive_package_bindings,
        vec![binding_one]
    );
    committed_two.validate_for_session(session_id).unwrap();

    generation_two.close().await;
    drop(generation_two);
    let (exact_context, exact_binding) = next_cognitive_context("0.3.0", 9, 0xcc);
    assert_eq!(exact_binding, binding_two);
    let resumed = agent
        .resume_session_async(
            session_id,
            replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                .with_llm_client(Arc::new(StaticStreamingClient::new("resumed")))
                .with_cognitive_context(exact_context),
        )
        .await
        .unwrap();
    assert_eq!(resumed.cognitive_package_binding(), Some(&binding_two));
    assert_eq!(
        resumed
            .run_store
            .records()
            .await
            .iter()
            .flat_map(|record| &record.events)
            .filter(|event| matches!(
                event.event,
                crate::agent::AgentEvent::CognitiveContextBound { .. }
            ))
            .count(),
        1
    );
}
