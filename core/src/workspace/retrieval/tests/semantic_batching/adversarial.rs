use super::*;

#[tokio::test]
async fn split_file_is_not_published_until_every_batch_is_valid() {
    let catalog = populated_catalog(
        &[(
            "src/split.rs".to_owned(),
            "first\nsecond\nthird\n".to_owned(),
        )],
        ChunkingConfig {
            max_lines: 1,
            max_bytes: 64,
            max_chunks_per_file: 8,
        },
    );
    let provider = RecordingProvider::gated();
    let embedding = EmbeddingExecutorConfig {
        max_batch_inputs: 2,
        ..EmbeddingExecutorConfig::default()
    };
    let runtime = start_runtime(catalog, provider.clone(), embedding);

    provider.wait_for_calls(1).await;
    provider.release(1);
    provider.wait_for_calls(2).await;
    let between_batches = runtime.status();
    assert_eq!(between_batches.indexed_files, 0);
    assert_eq!(between_batches.indexed_chunks, 0);
    assert_eq!(between_batches.vector_records, 0);

    provider.release(1);
    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;
    assert_eq!(ready.indexed_files, 1);
    assert_eq!(ready.indexed_chunks, 3);
    assert_eq!(provider.batch_sizes(), vec![2, 1]);
    assert_eq!(ready.batching.document_batches, 2);
    assert_eq!(ready.batching.batch_limit_lower_bound, 2);
    runtime.close().await;
}

#[tokio::test]
async fn a_later_failed_batch_preserves_an_already_valid_file() {
    let catalog = populated_catalog(
        &[
            ("a.rs".to_owned(), "complete\n".to_owned()),
            ("b.rs".to_owned(), "first\nsecond\n".to_owned()),
        ],
        ChunkingConfig {
            max_lines: 1,
            max_bytes: 64,
            max_chunks_per_file: 8,
        },
    );
    let provider = RecordingProvider::with_failures(&[1]);
    let embedding = EmbeddingExecutorConfig {
        max_batch_inputs: 2,
        max_retries: 0,
        ..EmbeddingExecutorConfig::default()
    };
    let runtime = start_runtime(catalog, provider.clone(), embedding);

    let degraded = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Degraded && status.queue_depth == 0
    })
    .await;

    assert_eq!(provider.batch_sizes(), vec![2, 1]);
    assert_eq!(degraded.eligible_files, 2);
    assert_eq!(degraded.indexed_files, 1);
    assert_eq!(degraded.indexed_chunks, 1);
    assert_eq!(degraded.failed_files, 1);
    assert_eq!(degraded.coverage_bps, 5_000);
    assert_eq!(degraded.batching.document_batches, 2);
    assert_eq!(degraded.batching.document_provider_requests, 2);
    runtime.close().await;
}

#[tokio::test]
async fn malformed_later_response_cannot_discard_an_already_valid_file() {
    let catalog = populated_catalog(
        &[
            ("a.rs".to_owned(), "complete\n".to_owned()),
            ("b.rs".to_owned(), "first\nsecond\n".to_owned()),
        ],
        ChunkingConfig {
            max_lines: 1,
            max_bytes: 64,
            max_chunks_per_file: 8,
        },
    );
    let provider = RecordingProvider::with_malformed_responses(&[1]);
    let runtime = start_runtime(
        catalog,
        provider.clone(),
        EmbeddingExecutorConfig {
            max_batch_inputs: 2,
            max_retries: 0,
            ..EmbeddingExecutorConfig::default()
        },
    );

    let degraded = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Degraded && status.queue_depth == 0
    })
    .await;

    assert_eq!(provider.batch_sizes(), vec![2, 1]);
    assert_eq!(degraded.indexed_files, 1);
    assert_eq!(degraded.indexed_chunks, 1);
    assert_eq!(degraded.failed_files, 1);
    assert_eq!(degraded.vector_records, 1);
    runtime.close().await;
}

#[tokio::test]
async fn metrics_count_physical_provider_retries_separately_from_logical_batches() {
    let catalog = populated_catalog(
        &[("a.rs".to_owned(), "retry once\n".to_owned())],
        ChunkingConfig::default(),
    );
    let provider = RecordingProvider::with_failures(&[0]);
    let runtime = start_runtime(
        catalog,
        provider.clone(),
        EmbeddingExecutorConfig {
            base_retry_delay: Duration::ZERO,
            max_retry_delay: Duration::ZERO,
            ..EmbeddingExecutorConfig::default()
        },
    );

    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;

    assert_eq!(provider.call_count(), 2);
    assert_eq!(ready.batching.document_batches, 1);
    assert_eq!(ready.batching.document_provider_requests, 2);
    assert_eq!(ready.batching.batch_limit_lower_bound, 1);
    runtime.close().await;
}

#[tokio::test]
async fn catalog_update_cancels_the_whole_batch_and_preserves_stable_sibling_ids() {
    let catalog = populated_catalog(
        &[
            ("a.rs".to_owned(), "generation one\n".to_owned()),
            ("b.rs".to_owned(), "stable sibling\n".to_owned()),
        ],
        ChunkingConfig::default(),
    );
    let provider = RecordingProvider::gated();
    let runtime = start_runtime(
        Arc::clone(&catalog),
        provider.clone(),
        EmbeddingExecutorConfig::default(),
    );
    provider.wait_for_calls(1).await;

    catalog
        .replace_file(
            &WorkspacePath::from_normalized("a.rs"),
            Some("rust"),
            2,
            "generation two\n",
        )
        .unwrap();
    provider.wait_for_calls(2).await;

    assert!(provider.request_was_cancelled(0));
    assert_eq!(runtime.status().indexed_files, 0);
    let request_ids = provider.request_ids();
    assert_eq!(request_ids[0].len(), 2);
    assert_eq!(request_ids[1].len(), 2);
    assert_ne!(request_ids[0][0], request_ids[1][0]);
    assert_eq!(request_ids[0][1], request_ids[1][1]);

    provider.release(1);
    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready && status.source_revision == 2
    })
    .await;
    assert_eq!(ready.indexed_files, 2);
    assert_eq!(ready.batching.document_provider_requests, 1);
    runtime.close().await;
}
