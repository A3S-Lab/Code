//! S-CM-02: thirty-two semantic publications, half rejected before CAS.

#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_memory::repository::{MemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorSearchRequest,
};
use async_trait::async_trait;
use refresh_support::{create_node, namespace, semantic, ALPHA, BETA, GAMMA};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const ATTEMPTS: usize = 32;
const NODE_ID_LABEL: &str = "a3s.memory.semantic.node_id";
const DELTA: &str = "Seal the violet audit trail after the failed refresh.";

struct FlakyProvider {
    fail: AtomicBool,
    calls: AtomicUsize,
}

impl FlakyProvider {
    fn descriptor() -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "generation-soak-v1", 2)
            .with_revision("fixture-r1")
            .with_normalization(EmbeddingNormalization::Unit)
    }
}

#[async_trait]
impl EmbeddingProvider for FlakyProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        Self::descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(EmbeddingProviderError::Unavailable { retry_after: None });
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                let values = match input.text() {
                    ALPHA | GAMMA => vec![1.0, 0.0],
                    BETA | DELTA => vec![0.0, 1.0],
                    _ => return Err(EmbeddingProviderError::InvalidRequest),
                };
                Ok(EmbeddingVector::new(input.id(), values))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

fn generation_of(node_ids: &[String]) -> &'static str {
    if node_ids.iter().all(|id| id.starts_with("gen-a-")) {
        "gen-a"
    } else if node_ids.iter().all(|id| id.starts_with("gen-b-")) {
        "gen-b"
    } else {
        panic!("torn semantic generation: {node_ids:?}");
    }
}

#[tokio::test]
#[ignore = "S-CM-02 soak: 32 semantic publications with concurrent recall"]
async fn soak_failed_semantic_publications_do_not_tear_generations() {
    let namespace = namespace("generation-soak");
    let repository = Arc::new(a3s_memory::repository::InMemoryRepository::new());
    let specs = [
        ("create-gen-a-0", "gen-a-0", ALPHA),
        ("create-gen-a-1", "gen-a-1", BETA),
        ("create-gen-b-0", "gen-b-0", GAMMA),
        ("create-gen-b-1", "gen-b-1", DELTA),
    ];
    for (key, id, content) in specs {
        create_node(
            repository.as_ref(),
            &namespace,
            key,
            id,
            MemoryStatus::Active,
            content,
            1,
        )
        .await;
    }
    let mut nodes = Vec::new();
    for (_, id, _) in specs {
        nodes.push(
            repository
                .get(&namespace, id)
                .await
                .unwrap()
                .expect("seeded node"),
        );
    }
    let generation_a = vec![nodes[0].clone(), nodes[1].clone()];
    let generation_b = vec![nodes[2].clone(), nodes[3].clone()];

    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(FlakyProvider {
        fail: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let semantic = semantic(
        provider.clone(),
        EmbeddingExecutorConfig {
            max_retries: 0,
            ..EmbeddingExecutorConfig::default()
        },
        index.clone(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let reader = {
        let index = index.clone();
        let stop = stop.clone();
        let reads = reads.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                let result = index
                    .search(VectorSearchRequest::new(vec![1.0, 0.0], 8))
                    .await
                    .expect("search published snapshot");
                let node_ids: Vec<String> = result
                    .hits
                    .iter()
                    .filter_map(|hit| hit.labels.get(NODE_ID_LABEL).cloned())
                    .collect();
                if !node_ids.is_empty() {
                    let generation = generation_of(&node_ids);
                    assert_eq!(
                        node_ids.len(),
                        2,
                        "reader saw a partial {generation} snapshot"
                    );
                    reads.fetch_add(1, Ordering::SeqCst);
                }
                tokio::task::yield_now().await;
            }
        })
    };

    let mut successes = 0usize;
    for attempt in 0..ATTEMPTS {
        let fail = attempt % 2 == 1;
        provider.fail.store(fail, Ordering::SeqCst);
        let before = index.status().revision;
        let nodes = if successes % 2 == 0 {
            generation_a.clone()
        } else {
            generation_b.clone()
        };
        let published = semantic
            .replace_namespace(&namespace, nodes, CancellationToken::new())
            .await;
        let after = index.status().revision;
        if fail {
            assert!(published.is_err(), "attempt {attempt} should fail closed");
            assert_eq!(after, before, "failed attempt appended a generation");
        } else {
            published.expect("successful publication");
            assert_eq!(after.value(), before.value() + 1);
            successes += 1;
        }
    }
    stop.store(true, Ordering::SeqCst);
    reader.await.expect("reader task");

    assert_eq!(successes, ATTEMPTS / 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), ATTEMPTS);
    assert_eq!(index.status().revision.value(), successes as u64);
    assert_eq!(index.status().record_count, 2);
    assert!(
        reads.load(Ordering::SeqCst) > 0,
        "reader never observed a generation"
    );

    let final_result = index
        .search(VectorSearchRequest::new(vec![1.0, 0.0], 8))
        .await
        .unwrap();
    let final_ids: Vec<String> = final_result
        .hits
        .iter()
        .filter_map(|hit| hit.labels.get(NODE_ID_LABEL).cloned())
        .collect();
    assert_eq!(generation_of(&final_ids), "gen-b");
    assert_eq!(final_ids.len(), 2);
}
