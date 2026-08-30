#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

#[path = "durable_memory_semantic_refresh/checkpoint_support.rs"]
mod checkpoint_support;

use a3s_code_core::memory::{ScheduledSemanticRefresh, SemanticRefreshRunOutcome};
use a3s_code_core::{
    DurableMemorySemanticRefreshCheckpoint, DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1,
};
use a3s_memory::repository::{InMemoryRepository, MemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorMutationConsistency,
};
use checkpoint_support::*;
use refresh_support::*;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn serialized_checkpoint_recovers_with_one_snapshot_and_no_provider_or_publication_work() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DurableMemorySemanticRefreshCheckpoint>();

    let namespace = namespace("scheduled-checkpoint-recovery");
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
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(CountingProvider::default());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let durable = durable(repository, namespace, provider.clone(), vector_index);

    let first_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let first = start_runtime("checkpoint-first", durable.clone(), first_schedule.clone());
    advance_until(first.as_ref(), 1).await;
    let first_receipt = first_schedule.last_receipt().expect("first receipt");
    let checkpoint = first_receipt.checkpoint();
    assert_eq!(
        checkpoint.schema(),
        DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1
    );
    assert!(checkpoint.index_change_token().is_some());
    let encoded = serde_json::to_string(&checkpoint).unwrap();
    assert!(!encoded.contains("sourceChangeToken"));
    assert!(encoded.contains("indexChangeToken"));
    assert!(!encoded.contains(ALPHA));
    assert!(!encoded.contains("alpha"));
    let decoded: DurableMemorySemanticRefreshCheckpoint = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, checkpoint);

    let mut unknown = serde_json::to_value(&checkpoint).unwrap();
    unknown["sourceChangeToken"] = serde_json::json!({"profile": "forged", "sequence": 1});
    assert!(serde_json::from_value::<DurableMemorySemanticRefreshCheckpoint>(unknown).is_err());
    let mut unsupported = serde_json::to_value(&checkpoint).unwrap();
    unsupported["schema"] = serde_json::json!("a3s.code.memory.semantic-refresh-checkpoint.v2");
    let unsupported =
        serde_json::from_value::<DurableMemorySemanticRefreshCheckpoint>(unsupported).unwrap();
    assert!(
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), unsupported,)
            .is_err()
    );
    let mut mismatched_revision = serde_json::to_value(&checkpoint).unwrap();
    mismatched_revision["indexChangeToken"]["revision"] = serde_json::json!(u64::MAX);
    let mismatched_revision =
        serde_json::from_value::<DurableMemorySemanticRefreshCheckpoint>(mismatched_revision)
            .unwrap();
    assert!(ScheduledSemanticRefresh::try_new_with_checkpoint(
        Duration::from_secs(1),
        mismatched_revision,
    )
    .is_err());

    first.close().await;
    let published_status = index.status();
    assert_eq!(provider.calls(), 1);
    assert_eq!(provider.inputs(), 1);

    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), decoded).unwrap();
    let recovered = start_runtime("checkpoint-recovered", durable, recovered_schedule.clone());
    assert!(recovered_schedule.last_receipt().is_none());
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 0);
    assert_eq!(metrics.unchanged_runs(), 1);
    assert_eq!(metrics.total_source_change_token_requests(), 2);
    assert_eq!(metrics.total_source_change_token_observations(), 2);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_embedding_inputs(), 0);
    assert_eq!(metrics.total_provider_requests(), 0);
    assert_eq!(metrics.total_publication_attempts(), 0);
    assert_eq!(index.status(), published_status);
    assert_eq!(provider.calls(), 1);
    assert!(recovered_schedule
        .last_receipt()
        .expect("recovered receipt")
        .source_change_token()
        .is_some());

    advance_until(recovered.as_ref(), 2).await;
    let steady = recovered_schedule.metrics();
    assert_eq!(steady.unchanged_runs(), 2);
    assert_eq!(steady.total_source_change_token_requests(), 3);
    assert_eq!(steady.total_source_snapshot_requests(), 1);
    assert_eq!(steady.total_provider_requests(), 0);
    assert_eq!(
        steady.last_run().expect("steady run").outcome(),
        SemanticRefreshRunOutcome::Unchanged
    );
    recovered.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_checkpoint_recovery_retains_unverified_evidence_for_retry() {
    let namespace = namespace("scheduled-checkpoint-retry");
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
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(CountingProvider::default());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let first_durable = durable(
        repository.clone(),
        namespace.clone(),
        provider.clone(),
        vector_index.clone(),
    );
    let first_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let first = start_runtime(
        "checkpoint-retry-first",
        first_durable,
        first_schedule.clone(),
    );
    advance_until(first.as_ref(), 1).await;
    let checkpoint = first_schedule
        .last_receipt()
        .expect("first receipt")
        .checkpoint();
    first.close().await;
    let published_status = index.status();

    let flaky_repository: Arc<dyn MemoryRepository> =
        Arc::new(SnapshotOnlyRepository::failing_once(repository));
    let recovered_durable = durable(flaky_repository, namespace, provider.clone(), vector_index);
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-retry-recovered",
        recovered_durable,
        recovered_schedule.clone(),
    );

    advance_until_failure(recovered.as_ref(), 1).await;
    let failed = recovered_schedule.metrics();
    assert_eq!(failed.failed_runs(), 1);
    assert_eq!(failed.total_source_snapshot_requests(), 1);
    assert_eq!(failed.total_provider_requests(), 0);
    assert_eq!(failed.total_publication_attempts(), 0);
    assert!(recovered_schedule.last_receipt().is_none());

    advance_until(recovered.as_ref(), 1).await;
    let retried = recovered_schedule.metrics();
    assert_eq!(retried.failed_runs(), 1);
    assert_eq!(retried.unchanged_runs(), 1);
    assert_eq!(retried.total_source_snapshot_requests(), 2);
    assert_eq!(retried.total_provider_requests(), 0);
    assert_eq!(retried.total_publication_attempts(), 0);
    assert_eq!(provider.calls(), 1);
    assert_eq!(index.status(), published_status);
    assert!(recovered_schedule.last_receipt().is_some());
    recovered.close().await;
}

#[tokio::test(start_paused = true)]
async fn checkpoint_never_trusts_a_colliding_token_from_an_unrelated_repository_history() {
    let namespace = namespace("scheduled-checkpoint-token-collision");
    let first_repository = Arc::new(InMemoryRepository::new());
    create_node(
        first_repository.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(CountingProvider::default());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let first_durable = durable(
        first_repository,
        namespace.clone(),
        provider.clone(),
        vector_index.clone(),
    );
    let first_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let first = start_runtime(
        "checkpoint-collision-first",
        first_durable,
        first_schedule.clone(),
    );
    advance_until(first.as_ref(), 1).await;
    let first_receipt = first_schedule.last_receipt().expect("first receipt");
    assert_eq!(
        first_receipt
            .source_change_token()
            .expect("first token")
            .sequence(),
        1
    );
    let checkpoint = first_receipt.checkpoint();
    let old_digest = first_receipt.source_snapshot_digest().to_string();
    first.close().await;

    let unrelated_repository = Arc::new(InMemoryRepository::new());
    create_node(
        unrelated_repository.as_ref(),
        &namespace,
        "create-gamma",
        "gamma",
        MemoryStatus::Active,
        GAMMA,
        1,
    )
    .await;
    assert_eq!(
        unrelated_repository
            .namespace_change_token(&namespace)
            .await
            .unwrap()
            .expect("unrelated token")
            .sequence(),
        1,
        "the adversarial repositories must expose the same token value"
    );
    let unrelated_durable = durable(
        unrelated_repository,
        namespace,
        provider.clone(),
        vector_index,
    );
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-collision-recovered",
        unrelated_durable.clone(),
        recovered_schedule.clone(),
    );
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 1);
    assert_eq!(metrics.unchanged_runs(), 0);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_embedding_inputs(), 1);
    assert_eq!(metrics.total_provider_requests(), 1);
    assert_eq!(metrics.total_publication_attempts(), 1);
    assert_eq!(provider.calls(), 2);
    assert_eq!(provider.inputs(), 2);
    let recovered_receipt = recovered_schedule.last_receipt().expect("rebuilt receipt");
    assert_ne!(recovered_receipt.source_snapshot_digest(), old_digest);
    let gamma = unrelated_durable.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(gamma.hits.len(), 1);
    assert_eq!(gamma.hits[0].node_id, "gamma");
    recovered.close().await;
}

#[tokio::test(start_paused = true)]
async fn checkpoint_never_trusts_colliding_status_from_an_unrelated_vector_history() {
    let namespace = namespace("scheduled-checkpoint-index-collision");
    let source_repository = Arc::new(InMemoryRepository::new());
    create_node(
        source_repository.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let provider = Arc::new(CountingProvider::default());
    let source_index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let source_vector_index: Arc<dyn VectorIndex> = source_index.clone();
    let source_durable = durable(
        source_repository.clone(),
        namespace.clone(),
        provider.clone(),
        source_vector_index,
    );
    let source_receipt = source_durable
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let checkpoint = source_receipt.checkpoint();

    let unrelated_repository = Arc::new(InMemoryRepository::new());
    create_node(
        unrelated_repository.as_ref(),
        &namespace,
        "create-beta",
        "alpha",
        MemoryStatus::Active,
        BETA,
        1,
    )
    .await;
    let unrelated_index =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let unrelated_vector_index: Arc<dyn VectorIndex> = unrelated_index.clone();
    let unrelated_durable = durable(
        unrelated_repository,
        namespace.clone(),
        provider.clone(),
        unrelated_vector_index,
    );
    let unrelated_receipt = unrelated_durable
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        unrelated_receipt.index_status(),
        source_receipt.index_status(),
        "the adversarial indexes must expose colliding logical status"
    );
    assert_ne!(
        unrelated_index.change_token(),
        source_index.change_token(),
        "independent vector histories must remain distinguishable"
    );

    let recovered_vector_index: Arc<dyn VectorIndex> = unrelated_index;
    let recovered_durable = durable(
        source_repository,
        namespace,
        provider.clone(),
        recovered_vector_index,
    );
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-index-collision-recovered",
        recovered_durable.clone(),
        recovered_schedule.clone(),
    );
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 1);
    assert_eq!(metrics.unchanged_runs(), 0);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_embedding_inputs(), 1);
    assert_eq!(metrics.total_publication_attempts(), 1);
    assert_eq!(provider.calls(), 3);
    let alpha = recovered_durable.preview_recall(ALPHA_QUERY).await.unwrap();
    assert_eq!(alpha.hits.len(), 1);
    assert_eq!(alpha.hits[0].node_id, "alpha");
    recovered.close().await;
}

#[tokio::test(start_paused = true)]
async fn checkpoint_recovery_preserves_snapshot_only_repository_compatibility() {
    let namespace = namespace("scheduled-checkpoint-snapshot-only");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(CountingProvider::default());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let first_durable = durable(
        inner.clone(),
        namespace.clone(),
        provider.clone(),
        vector_index.clone(),
    );
    let first_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let first = start_runtime(
        "checkpoint-fallback-first",
        first_durable,
        first_schedule.clone(),
    );
    advance_until(first.as_ref(), 1).await;
    let checkpoint = first_schedule
        .last_receipt()
        .expect("first receipt")
        .checkpoint();
    first.close().await;
    let published_status = index.status();

    let snapshot_only: Arc<dyn MemoryRepository> = Arc::new(SnapshotOnlyRepository::new(inner));
    let recovered_durable = durable(snapshot_only, namespace, provider.clone(), vector_index);
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-fallback-recovered",
        recovered_durable,
        recovered_schedule.clone(),
    );
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 0);
    assert_eq!(metrics.unchanged_runs(), 1);
    assert_eq!(metrics.total_source_change_token_requests(), 1);
    assert_eq!(metrics.total_source_change_token_observations(), 0);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_provider_requests(), 0);
    assert_eq!(metrics.total_publication_attempts(), 0);
    assert_eq!(provider.calls(), 1);
    assert_eq!(index.status(), published_status);
    assert!(recovered_schedule
        .last_receipt()
        .expect("fallback receipt")
        .source_change_token()
        .is_none());
    recovered.close().await;
}

#[tokio::test(start_paused = true)]
async fn checkpoint_without_vector_history_continuity_rebuilds_instead_of_skipping() {
    let namespace = namespace("scheduled-checkpoint-no-vector-token");
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
    let inner_index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let no_token_index = Arc::new(NoChangeTokenVectorIndex::new(inner_index.clone()));
    let vector_index: Arc<dyn VectorIndex> = no_token_index;
    let provider = Arc::new(CountingProvider::default());
    let durable = durable(repository, namespace, provider.clone(), vector_index);

    let first_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let first = start_runtime(
        "checkpoint-no-vector-token-first",
        durable.clone(),
        first_schedule.clone(),
    );
    advance_until(first.as_ref(), 1).await;
    let checkpoint = first_schedule
        .last_receipt()
        .expect("first receipt")
        .checkpoint();
    assert!(checkpoint.index_change_token().is_none());
    first.close().await;
    assert_eq!(provider.calls(), 1);
    assert_eq!(inner_index.status().revision.value(), 1);

    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(Duration::from_secs(1), checkpoint)
            .unwrap();
    let recovered = start_runtime(
        "checkpoint-no-vector-token-recovered",
        durable,
        recovered_schedule.clone(),
    );
    advance_until(recovered.as_ref(), 1).await;

    let metrics = recovered_schedule.metrics();
    assert_eq!(metrics.published_runs(), 1);
    assert_eq!(metrics.unchanged_runs(), 0);
    assert_eq!(metrics.total_source_snapshot_requests(), 1);
    assert_eq!(metrics.total_embedding_inputs(), 1);
    assert_eq!(metrics.total_provider_requests(), 1);
    assert_eq!(metrics.total_publication_attempts(), 1);
    assert_eq!(provider.calls(), 2);
    assert_eq!(inner_index.status().revision.value(), 2);
    assert!(recovered_schedule
        .last_receipt()
        .expect("rebuilt receipt")
        .index_change_token()
        .is_none());
    recovered.close().await;
}
