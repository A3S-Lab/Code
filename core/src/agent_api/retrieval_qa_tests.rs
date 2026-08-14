use super::{Agent, AgentSession, SessionOptions};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::store::{MemorySessionStore, SessionStore};
use crate::workspace::{
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceSemanticSearchRequest,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const SAFE_SENTINEL: &str = "WSR_QA_SAFE_SOURCE_20260814";
const ENV_SENTINEL: &str = "WSR_QA_ENV_SECRET_20260814";
const ACL_SENTINEL: &str = "WSR_QA_ACL_SECRET_20260814";
const GIT_SENTINEL: &str = "WSR_QA_GIT_SECRET_20260814";
const GENERATED_SENTINEL: &str = "WSR_QA_GENERATED_SECRET_20260814";
const BINARY_SENTINEL: &str = "WSR_QA_BINARY_SECRET_20260814";
const OVERSIZED_SENTINEL: &str = "WSR_QA_OVERSIZED_SECRET_20260814";
const SWAP_SENTINEL: &str = "WSR_QA_HARDLINK_SWAP_SECRET_20260814";

#[tokio::test]
async fn embedding_egress_contains_only_path_and_identity_admitted_source() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_egress_fixture(workspace.path());
    let provider = Arc::new(RecordingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let options = SessionOptions::new()
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap();

    wait_for_degraded_files(&session, 2, 1, 1).await;
    let submitted = provider.document_texts().join("\n");
    assert!(submitted.contains(SAFE_SENTINEL));
    for rejected in [
        ENV_SENTINEL,
        ACL_SENTINEL,
        GIT_SENTINEL,
        GENERATED_SENTINEL,
        BINARY_SENTINEL,
        OVERSIZED_SENTINEL,
    ] {
        assert!(
            !submitted.contains(rejected),
            "sensitive sentinel reached the embedding provider: {rejected}"
        );
    }
    session.close().await;
}

#[tokio::test]
async fn hard_link_swap_after_admission_never_reaches_the_embedding_provider() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    let admitted = workspace.path().join("src/admitted.rs");
    let credential = workspace.path().join(".env");
    std::fs::write(&admitted, "pub fn initially_safe() {}\n").unwrap();
    std::fs::write(&credential, format!("TOKEN={SWAP_SENTINEL}\n")).unwrap();

    let provider = Arc::new(RecordingProvider::new());
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = retrieval_session(&agent, workspace.path(), Arc::clone(&provider)).await;
    wait_for_ready_files(&session, 1).await;

    std::fs::remove_file(&admitted).unwrap();
    std::fs::hard_link(&credential, &admitted).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = session.workspace_retrieval_status();
            if status.catalog_files == 0 && status.indexed_files == 0 && status.vector_records == 0
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "hard-link swap did not converge to an empty admitted catalog: {:?}",
            session.workspace_retrieval_status()
        )
    });
    assert!(provider
        .document_texts()
        .iter()
        .all(|text| !text.contains(SWAP_SENTINEL)));
    session.close().await;
}

#[tokio::test]
async fn sessions_do_not_share_retrieval_results_status_or_cancellation() {
    let workspace_a = tempfile::tempdir().unwrap();
    let workspace_b = tempfile::tempdir().unwrap();
    std::fs::write(workspace_a.path().join("only-a.rs"), "workspace alpha\n").unwrap();
    std::fs::write(workspace_b.path().join("only-b.rs"), "workspace beta\n").unwrap();
    let provider = Arc::new(RecordingProvider::new());
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    let session_a = retrieval_session(&agent, workspace_a.path(), Arc::clone(&provider)).await;
    let session_b = retrieval_session(&agent, workspace_b.path(), Arc::clone(&provider)).await;
    wait_for_ready_files(&session_a, 1).await;
    wait_for_ready_files(&session_b, 1).await;

    let result_a = session_a
        .semantic_search(WorkspaceSemanticSearchRequest::new("workspace"))
        .await
        .unwrap();
    let result_b = session_b
        .semantic_search(WorkspaceSemanticSearchRequest::new("workspace"))
        .await
        .unwrap();
    assert!(result_a
        .hits
        .iter()
        .all(|hit| hit.chunk.path.as_ref() == "only-a.rs"));
    assert!(result_b
        .hits
        .iter()
        .all(|hit| hit.chunk.path.as_ref() == "only-b.rs"));

    session_a.close().await;
    assert_eq!(
        session_a.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Closed
    );
    assert_eq!(
        session_b.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Ready
    );
    let after_peer_close = session_b
        .semantic_search(WorkspaceSemanticSearchRequest::new("workspace"))
        .await
        .unwrap();
    assert_eq!(after_peer_close.hits[0].chunk.path.as_ref(), "only-b.rs");
    session_b.close().await;
}

#[tokio::test]
async fn persisted_session_snapshot_excludes_source_vectors_and_provider_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let source_sentinel = "WSR_QA_SNAPSHOT_SOURCE_20260814";
    std::fs::write(
        workspace.path().join("snapshot.rs"),
        format!("pub const MARKER: &str = \"{source_sentinel}\";\n"),
    )
    .unwrap();
    let provider = Arc::new(RecordingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider;
    let store = Arc::new(MemorySessionStore::new());
    let store_port: Arc<dyn SessionStore> = store.clone();
    let options = SessionOptions::new()
        .with_session_id("retrieval-persistence-qa")
        .with_session_store(store_port)
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap();
    wait_for_ready_files(&session, 1).await;

    session.save().await.unwrap();
    let snapshot = store
        .load_snapshot("retrieval-persistence-qa")
        .await
        .unwrap()
        .unwrap();
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains(source_sentinel));
    assert!(!serialized.contains("qa-recording-provider"));
    assert!(!serialized.contains("qa-recording-model"));
    assert!(!serialized.contains("workspace_retrieval"));
    session.close().await;
}

#[tokio::test]
async fn repeated_session_lifecycle_releases_every_ephemeral_index() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("soak.rs"), "session lifecycle soak\n").unwrap();
    let provider = Arc::new(RecordingProvider::new());
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    for _ in 0..16 {
        let session = retrieval_session(&agent, workspace.path(), Arc::clone(&provider)).await;
        wait_for_ready_files(&session, 1).await;
        assert!(session.workspace_retrieval_status().vector_records > 0);
        session.close().await;
        let closed = session.workspace_retrieval_status();
        assert_eq!(closed.phase, WorkspaceRetrievalPhase::Closed);
        assert_eq!(closed.vector_records, 0);
        assert_eq!(closed.vector_bytes, 0);
    }
}

async fn retrieval_session(
    agent: &Agent,
    workspace: &std::path::Path,
    provider: Arc<RecordingProvider>,
) -> AgentSession {
    let provider: Arc<dyn EmbeddingProvider> = provider;
    agent
        .session_async(
            workspace.to_string_lossy(),
            Some(
                SessionOptions::new()
                    .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider)),
            ),
        )
        .await
        .unwrap()
}

async fn wait_for_ready_files(session: &AgentSession, expected_files: usize) {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = session.workspace_retrieval_status();
            if status.phase == WorkspaceRetrievalPhase::Ready
                && status.eligible_files == expected_files
                && status.indexed_files == expected_files
            {
                return;
            }
            assert_ne!(status.phase, WorkspaceRetrievalPhase::Closed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        panic!(
            "workspace retrieval did not reach the expected ready state: {:?}",
            session.workspace_retrieval_status()
        );
    }
}

async fn wait_for_degraded_files(
    session: &AgentSession,
    expected_eligible: usize,
    expected_indexed: usize,
    expected_failed: usize,
) {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = session.workspace_retrieval_status();
            if status.phase == WorkspaceRetrievalPhase::Degraded
                && status.eligible_files == expected_eligible
                && status.indexed_files == expected_indexed
                && status.failed_files == expected_failed
            {
                return;
            }
            assert_ne!(status.phase, WorkspaceRetrievalPhase::Closed);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        panic!(
            "workspace retrieval did not reach the expected degraded state: {:?}",
            session.workspace_retrieval_status()
        );
    }
}

fn write_workspace_egress_fixture(root: &std::path::Path) {
    for directory in ["src", ".a3s", ".git", "target"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("src/lib.rs"),
        format!("pub const SAFE: &str = \"{SAFE_SENTINEL}\";\n"),
    )
    .unwrap();
    std::fs::write(root.join(".env"), format!("TOKEN={ENV_SENTINEL}\n")).unwrap();
    std::fs::hard_link(root.join(".env"), root.join("src/apparently-safe.rs")).unwrap();
    std::fs::write(
        root.join(".a3s/config.acl"),
        format!("secret = \"{ACL_SENTINEL}\"\n"),
    )
    .unwrap();
    std::fs::write(root.join(".git/config"), GIT_SENTINEL).unwrap();
    std::fs::write(root.join("target/generated.rs"), GENERATED_SENTINEL).unwrap();
    std::fs::write(
        root.join("src/blob.bin"),
        [BINARY_SENTINEL.as_bytes(), b"\0binary"].concat(),
    )
    .unwrap();
    let mut oversized = OVERSIZED_SENTINEL.as_bytes().to_vec();
    oversized.resize(513 * 1024, b'x');
    std::fs::write(root.join("src/oversized.rs"), oversized).unwrap();
}

struct RecordingProvider {
    descriptor: EmbeddingProviderDescriptor,
    inputs: Mutex<Vec<(String, String)>>,
}

impl RecordingProvider {
    fn new() -> Self {
        Self {
            descriptor: EmbeddingProviderDescriptor::new(
                "qa-recording-provider",
                "qa-recording-model",
                8,
            ),
            inputs: Mutex::new(Vec::new()),
        }
    }

    fn document_texts(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id != "workspace-query")
            .map(|(_, text)| text.clone())
            .collect()
    }
}

#[async_trait]
impl EmbeddingProvider for RecordingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let mut recorded = self.inputs.lock().unwrap();
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                recorded.push((input.id().to_owned(), input.text().to_owned()));
                EmbeddingVector::new(input.id(), vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            })
            .collect();
        drop(recorded);
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}
