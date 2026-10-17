#![cfg(test)]

use super::{A3sCodeAgent, SessionSetup};
use a3s_code_core::{
    CodeConfig, EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, ModelConfig, ProviderConfig,
    WorkspaceRetrievalPhase,
};
use async_trait::async_trait;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn fixture_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/model".to_string()),
        providers: vec![ProviderConfig {
            name: "fixture".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            headers: Default::default(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "model".to_string(),
                name: "model".to_string(),
                family: "fixture".to_string(),
                api_key: None,
                base_url: None,
                headers: Default::default(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: Default::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn host_options(
    root: &Path,
    config: &CodeConfig,
    session_id: &str,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
) -> a3s_code_core::SessionOptions {
    let setup = SessionSetup {
        workspace: root.to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let store = root.join("store");
    let trust = root.join("hooks-trust.json");
    let options = A3sCodeAgent::core_session_options(session_id, setup.model_id, setup.effort);
    let mut slots = crate::host_context::merge_studio_host(
        options.prompt_slots.clone().unwrap_or_default(),
        &setup.host,
    );
    A3sCodeAgent::host_session_options_with_hook_roots(
        options,
        &mut slots,
        &setup,
        config,
        &store,
        None,
        Some(&trust),
        embedding,
    )
    .expect("host options")
}

fn capability_note(options: &a3s_code_core::SessionOptions) -> String {
    options
        .prompt_slots
        .as_ref()
        .and_then(|slots| slots.extra.clone())
        .expect("capability status")
}

fn search_surface(session: &a3s_code_core::AgentSession) -> (serde_json::Value, String) {
    let search = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "search")
        .expect("search tool");
    (
        search.parameters["properties"]["mode"]["enum"].clone(),
        search.description,
    )
}

struct BlockingEmbed {
    calls: AtomicUsize,
    called: Notify,
    cancellation: Mutex<Option<CancellationToken>>,
}

impl BlockingEmbed {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            called: Notify::new(),
            cancellation: Mutex::new(None),
        }
    }

    async fn wait_for_calls(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.calls.load(Ordering::Acquire) < expected {
                self.called.notified().await;
            }
        })
        .await
        .expect("embedding provider was not called");
    }

    fn request_was_cancelled(&self) -> bool {
        self.cancellation
            .lock()
            .expect("cancellation")
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
}

#[async_trait]
impl EmbeddingProvider for BlockingEmbed {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "blocked-v1", 2)
    }

    async fn embed(
        &self,
        _request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        *self.cancellation.lock().expect("cancellation") = Some(cancellation.clone());
        self.called.notify_waiters();
        cancellation.cancelled().await;
        Err(EmbeddingProviderError::Cancelled)
    }
}

struct FailingEmbed;

#[async_trait]
impl EmbeddingProvider for FailingEmbed {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "fail-v1", 2)
    }

    async fn embed(
        &self,
        _request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        Err(EmbeddingProviderError::InvalidRequest)
    }
}

async fn open_session(
    agent: &a3s_code_core::Agent,
    root: &Path,
    options: a3s_code_core::SessionOptions,
) -> a3s_code_core::AgentSession {
    tokio::time::timeout(
        Duration::from_secs(5),
        agent.session_async(root.to_string_lossy().to_string(), Some(options)),
    )
    .await
    .expect("session construction waited for embeddings")
    .expect("session")
}

#[tokio::test]
async fn retrieval_seam_stays_closed_until_a_provider_is_injected() {
    let config = fixture_config();
    let agent = a3s_code_core::Agent::from_config(config.clone())
        .await
        .expect("agent");

    let closed = tempfile::tempdir().expect("workspace");
    let store = closed.path().join("store");
    let setup = SessionSetup {
        workspace: closed.path().to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let closed_options =
        A3sCodeAgent::host_session_options("retrieval-seam-off", &setup, &config, &store)
            .expect("host options");
    assert!(closed_options.workspace_retrieval.is_none());
    let closed_note = capability_note(&closed_options);
    assert!(closed_note.contains("语义检索关闭"));
    assert!(!closed_note.contains("semantic, hybrid"));
    let closed_session = open_session(&agent, closed.path(), closed_options).await;
    assert_eq!(
        closed_session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Disabled
    );
    let (modes, description) = search_surface(&closed_session);
    assert_eq!(modes, serde_json::json!(["grep", "glob", "bm25"]));
    assert!(!description.contains("semantic"));
    assert!(!description.contains("hybrid"));
    closed_session.close().await;

    let blocked_root = tempfile::tempdir().expect("workspace");
    std::fs::write(
        blocked_root.path().join("lib.rs"),
        "pub fn semantic_probe() {}\n",
    )
    .expect("source");
    let blocking = Arc::new(BlockingEmbed::new());
    let blocking_port: Arc<dyn EmbeddingProvider> = blocking.clone();
    let blocked_options = host_options(
        blocked_root.path(),
        &config,
        "retrieval-seam-on",
        Some(blocking_port.clone()),
    );
    let attached = blocked_options
        .workspace_retrieval
        .as_ref()
        .expect("retrieval options")
        .embedding_provider();
    assert!(Arc::ptr_eq(&blocking_port, &attached));
    let blocked_note = capability_note(&blocked_options);
    assert!(blocked_note.contains("search modes: grep, glob, bm25, semantic, hybrid."));
    assert!(blocked_note
        .contains("If the index is degraded, semantic and hybrid leave the tool definition."));
    assert!(!blocked_note.contains("语义检索关闭"));
    let blocked_session = open_session(&agent, blocked_root.path(), blocked_options).await;
    assert_eq!(
        blocked_session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Building
    );
    let (modes, description) = search_surface(&blocked_session);
    assert_eq!(
        modes,
        serde_json::json!(["grep", "glob", "bm25", "semantic", "hybrid"])
    );
    assert!(description.contains("semantic"));
    assert!(description.contains("hybrid"));
    blocking.wait_for_calls(1).await;
    tokio::time::timeout(Duration::from_secs(5), blocked_session.close())
        .await
        .expect("close waited for the blocked embedding");
    assert!(blocking.request_was_cancelled());

    let failed_root = tempfile::tempdir().expect("workspace");
    std::fs::write(
        failed_root.path().join("lib.rs"),
        "pub fn semantic_probe() {}\n",
    )
    .expect("source");
    let failing: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbed);
    let failed_options = host_options(
        failed_root.path(),
        &config,
        "retrieval-seam-degraded",
        Some(failing),
    );
    let failed_session = open_session(&agent, failed_root.path(), failed_options).await;
    let degraded = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = failed_session.workspace_retrieval_status();
            if status.phase == WorkspaceRetrievalPhase::Degraded {
                return;
            }
            assert_ne!(status.phase, WorkspaceRetrievalPhase::Closed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if degraded.is_err() {
        panic!(
            "index did not degrade: {:?}",
            failed_session.workspace_retrieval_status()
        );
    }
    let (modes, description) = search_surface(&failed_session);
    assert_eq!(modes, serde_json::json!(["grep", "glob", "bm25"]));
    assert!(description.contains("bm25"));
    assert!(!description.contains("semantic"));
    assert!(!description.contains("hybrid"));
    failed_session.close().await;
}
