use super::super::*;
use super::support::{descriptor, executor, fast_config, input, FakeAction, FakeProvider};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn batches_by_count_and_bytes_and_restores_caller_order() {
    let provider = FakeProvider::new(
        descriptor(3),
        vec![
            FakeAction::Reverse,
            FakeAction::Reverse,
            FakeAction::Reverse,
        ],
    );
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 2,
        max_batch_text_bytes: 6,
        max_input_text_bytes: 4,
        max_request_inputs: 8,
        max_request_text_bytes: 32,
        ..fast_config()
    };
    let inputs = (0..5).map(|index| input(index, "abc")).collect::<Vec<_>>();

    let result = executor(&provider, config)
        .embed(inputs, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(
        provider.call_ids(),
        vec![
            vec!["chunk-0".to_owned(), "chunk-1".to_owned()],
            vec!["chunk-2".to_owned(), "chunk-3".to_owned()],
            vec!["chunk-4".to_owned()],
        ]
    );
    assert_eq!(result.batch_count, 3);
    assert_eq!(result.provider_attempts, 3);
    assert_eq!(
        result
            .vectors
            .iter()
            .map(|vector| vector.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["chunk-0", "chunk-1", "chunk-2", "chunk-3", "chunk-4"]
    );
}

#[tokio::test]
async fn later_batch_failure_returns_no_partial_execution() {
    let provider = FakeProvider::new(
        descriptor(2),
        vec![
            FakeAction::Success,
            FakeAction::Error(EmbeddingProviderError::Authentication),
        ],
    );
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 1,
        ..fast_config()
    };

    let result = executor(&provider, config)
        .embed(
            vec![input(0, "first"), input(1, "second")],
            CancellationToken::new(),
        )
        .await;

    assert_eq!(
        result.unwrap_err(),
        EmbeddingError::ProviderFailure {
            kind: EmbeddingFailureKind::Authentication,
            attempts: 1,
        }
    );
    assert_eq!(provider.call_count(), 2);
}

#[tokio::test]
async fn retries_only_typed_transient_failures_with_a_hard_attempt_bound() {
    let provider = FakeProvider::new(
        descriptor(2),
        vec![
            FakeAction::Error(EmbeddingProviderError::RateLimited {
                retry_after: Some(Duration::ZERO),
            }),
            FakeAction::Error(EmbeddingProviderError::Unavailable { retry_after: None }),
            FakeAction::Success,
        ],
    );
    let result = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.provider_attempts, 3);

    let provider = FakeProvider::new(
        descriptor(2),
        vec![
            FakeAction::Error(EmbeddingProviderError::Timeout),
            FakeAction::Error(EmbeddingProviderError::Timeout),
            FakeAction::Error(EmbeddingProviderError::Timeout),
        ],
    );
    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        EmbeddingError::RetriesExhausted {
            kind: EmbeddingFailureKind::Timeout,
            attempts: 3,
        }
    );
    assert_eq!(provider.call_count(), 3);

    let provider = FakeProvider::new(
        descriptor(2),
        vec![FakeAction::Error(EmbeddingProviderError::Authentication)],
    );
    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        EmbeddingError::ProviderFailure {
            kind: EmbeddingFailureKind::Authentication,
            attempts: 1,
        }
    );
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn detailed_request_metrics_count_retry_provider_boundary_work() {
    let provider = FakeProvider::new(
        descriptor(2),
        vec![
            FakeAction::Error(EmbeddingProviderError::RateLimited {
                retry_after: Some(Duration::ZERO),
            }),
            FakeAction::Success,
            FakeAction::Success,
        ],
    );
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 1,
        ..fast_config()
    };
    let metrics = EmbeddingProviderRequestMetrics::default();

    let result = executor(&provider, config)
        .embed_observed(
            vec![input(0, "first"), input(1, "second")],
            CancellationToken::new(),
            &metrics,
        )
        .await
        .unwrap();

    assert_eq!(result.provider_attempts, 3);
    assert_eq!(metrics.requests(), 3);
    assert_eq!(metrics.inputs(), 3);
    assert_eq!(metrics.input_bytes(), "first".len() * 2 + "second".len());
}

#[tokio::test]
async fn detailed_request_metrics_do_not_count_pre_provider_cancellation() {
    let provider = FakeProvider::new(descriptor(2), Vec::new());
    let metrics = EmbeddingProviderRequestMetrics::default();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = executor(&provider, fast_config())
        .embed_observed(vec![input(0, "source")], cancellation, &metrics)
        .await
        .unwrap_err();

    assert_eq!(error, EmbeddingError::Cancelled);
    assert_eq!(metrics.requests(), 0);
    assert_eq!(metrics.inputs(), 0);
    assert_eq!(metrics.input_bytes(), 0);
    assert_eq!(provider.call_count(), 0);
}

#[tokio::test]
async fn timeout_is_typed_and_bounded() {
    let provider = FakeProvider::new(
        descriptor(2),
        vec![FakeAction::Pending, FakeAction::Pending],
    );
    let config = EmbeddingExecutorConfig {
        max_retries: 1,
        request_timeout: Duration::from_millis(5),
        ..fast_config()
    };
    let error = executor(&provider, config)
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();

    assert_eq!(
        error,
        EmbeddingError::RetriesExhausted {
            kind: EmbeddingFailureKind::Timeout,
            attempts: 2,
        }
    );
    assert_eq!(provider.call_count(), 2);
    assert!(provider.request_was_cancelled(0));
    assert!(provider.request_was_cancelled(1));
}

#[tokio::test]
async fn pre_cancelled_request_never_calls_the_provider() {
    let provider = FakeProvider::new(descriptor(2), Vec::new());
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], cancellation)
        .await
        .unwrap_err();

    assert_eq!(error, EmbeddingError::Cancelled);
    assert_eq!(provider.call_count(), 0);
}

#[tokio::test]
async fn cancellation_reaches_an_active_provider_request() {
    let provider = FakeProvider::new(descriptor(2), vec![FakeAction::WaitForCancellation]);
    let execution = executor(&provider, fast_config());
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            execution
                .embed(vec![input(0, "source")], cancellation)
                .await
        }
    });
    provider.wait_for_calls(1).await;
    cancellation.cancel();

    assert_eq!(task.await.unwrap().unwrap_err(), EmbeddingError::Cancelled);
    assert!(provider.request_was_cancelled(0));
}

#[tokio::test]
async fn cancellation_interrupts_retry_after_without_an_extra_attempt() {
    let provider = FakeProvider::new(
        descriptor(2),
        vec![FakeAction::Error(EmbeddingProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(60)),
        })],
    );
    let config = EmbeddingExecutorConfig {
        max_retry_delay: Duration::from_secs(60),
        ..fast_config()
    };
    let execution = executor(&provider, config);
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            execution
                .embed(vec![input(0, "source")], cancellation)
                .await
        }
    });
    provider.wait_for_calls(1).await;
    cancellation.cancel();

    assert_eq!(task.await.unwrap().unwrap_err(), EmbeddingError::Cancelled);
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn vector_footprint_is_bounded_before_provider_calls_and_drives_batching() {
    let provider = FakeProvider::new(
        descriptor(4),
        vec![FakeAction::Success, FakeAction::Success],
    );
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 8,
        max_batch_vector_bytes: 32,
        max_request_vector_bytes: 64,
        ..fast_config()
    };
    let result = executor(&provider, config)
        .embed(
            vec![input(0, "a"), input(1, "b"), input(2, "c")],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.batch_count, 2);
    assert_eq!(provider.call_ids()[0], ["chunk-0", "chunk-1"]);
    assert_eq!(provider.call_ids()[1], ["chunk-2"]);

    let provider = FakeProvider::new(descriptor(4), Vec::new());
    let config = EmbeddingExecutorConfig {
        max_request_vector_bytes: 31,
        max_batch_vector_bytes: 31,
        ..fast_config()
    };
    let error = executor(&provider, config)
        .embed(vec![input(0, "a"), input(1, "b")], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        EmbeddingError::BudgetExceeded {
            resource: "request vector byte",
            requested: 32,
            limit: 31,
        }
    );
    assert_eq!(provider.call_count(), 0);
}
