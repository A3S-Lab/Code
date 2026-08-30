#![cfg(feature = "durable-memory-sqlite")]

#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/checkpoint_support.rs"]
mod checkpoint_support;

use a3s_code_core::memory::ScheduledSemanticRefresh;
use a3s_memory::repository::{InMemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    SqliteVectorIndex, VectorIndex, VectorIndexDescriptor, VectorMutationConsistency,
};
use checkpoint_support::*;
use refresh_support::*;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn checkpoint_recovery_reopens_the_same_durable_sqlite_index_history() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("semantic.sqlite3");
    let namespace = namespace("scheduled-checkpoint-sqlite-reopen");
    let repository = Arc::new(InMemoryRepository::new());
    create_node(
        repository.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let provider = Arc::new(CountingProvider::default());

    let first_index = Arc::new(
        SqliteVectorIndex::open(&path, VectorIndexDescriptor::new(2))
            .await
            .unwrap(),
    );
    let first_vector_index: Arc<dyn VectorIndex> = first_index.clone();
    let first_durable = durable(
        repository.clone(),
        namespace.clone(),
        provider.clone(),
        first_vector_index,
    );
    let first_receipt = first_durable
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let checkpoint = first_receipt.checkpoint();
    let published_status = first_receipt.index_status().clone();
    let published_token = first_receipt
        .index_change_token()
        .expect("durable index token")
        .clone();
    drop(first_durable);
    drop(first_index);

    let reopened = Arc::new(
        SqliteVectorIndex::open(&path, VectorIndexDescriptor::new(2))
            .await
            .unwrap(),
    );
    let reopened_observation = reopened.observe().await.unwrap();
    assert_eq!(reopened_observation.status, published_status);
    assert_eq!(reopened_observation.change_token, Some(published_token));
    let reopened_vector_index: Arc<dyn VectorIndex> = reopened.clone();
    let recovered_durable = durable(
        repository,
        namespace,
        provider.clone(),
        reopened_vector_index,
    );
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-sqlite-reopened",
        recovered_durable.clone(),
        recovered_schedule.clone(),
    );
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 0);
    assert_eq!(metrics.unchanged_runs(), 1);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_provider_requests(), 0);
    assert_eq!(metrics.total_publication_attempts(), 0);
    assert_eq!(provider.calls(), 1);
    assert_eq!(reopened.observe().await.unwrap().status, published_status);
    let preview = recovered_durable.preview_recall(ALPHA_QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_id, "alpha");
    recovered.close().await;
}
