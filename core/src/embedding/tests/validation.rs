use super::super::*;
use super::support::{descriptor, executor, fast_config, input, FakeAction, FakeProvider};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn rejects_descriptor_drift_partial_batches_dimensions_and_non_finite_values() {
    let base = descriptor(3);
    let drifted = EmbeddingProviderDescriptor::new("fake", "deterministic-v2", 3);
    let cases = vec![
        (
            FakeAction::Response(EmbeddingBatchResponse::new(
                drifted,
                vec![EmbeddingVector::new("chunk-0", vec![1.0, 2.0, 3.0])],
            )),
            EmbeddingError::DescriptorChanged,
        ),
        (
            FakeAction::Response(EmbeddingBatchResponse::new(base.clone(), Vec::new())),
            EmbeddingError::OutputCountMismatch {
                expected: 1,
                actual: 0,
            },
        ),
        (
            FakeAction::Response(EmbeddingBatchResponse::new(
                base.clone(),
                vec![EmbeddingVector::new("chunk-0", vec![1.0, 2.0])],
            )),
            EmbeddingError::DimensionMismatch {
                input_index: 0,
                expected: 3,
                actual: 2,
            },
        ),
        (
            FakeAction::Response(EmbeddingBatchResponse::new(
                base.clone(),
                vec![EmbeddingVector::new("chunk-0", vec![1.0, f32::NAN, 3.0])],
            )),
            EmbeddingError::NonFiniteValue {
                input_index: 0,
                position: 1,
            },
        ),
    ];

    for (action, expected) in cases {
        let provider = FakeProvider::new(base.clone(), vec![action]);
        let error = executor(&provider, fast_config())
            .embed(vec![input(0, "source")], CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error, expected);
    }
}

#[tokio::test]
async fn rejects_duplicate_and_unknown_output_identifiers() {
    let base = descriptor(2);
    let provider = FakeProvider::new(
        base.clone(),
        vec![FakeAction::Response(EmbeddingBatchResponse::new(
            base.clone(),
            vec![
                EmbeddingVector::new("chunk-0", vec![1.0, 2.0]),
                EmbeddingVector::new("chunk-0", vec![3.0, 4.0]),
            ],
        ))],
    );
    let error = executor(&provider, fast_config())
        .embed(
            vec![input(0, "first"), input(1, "second")],
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, EmbeddingError::DuplicateOutput { input_index: 0 });

    let provider = FakeProvider::new(
        base.clone(),
        vec![FakeAction::Response(EmbeddingBatchResponse::new(
            base,
            vec![EmbeddingVector::new("unknown", vec![1.0, 2.0])],
        ))],
    );
    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error, EmbeddingError::UnexpectedOutput);
}

#[tokio::test]
async fn validates_unit_normalization_contract() {
    let descriptor = descriptor(2).with_normalization(EmbeddingNormalization::Unit);
    let provider = FakeProvider::new(
        descriptor.clone(),
        vec![FakeAction::Response(EmbeddingBatchResponse::new(
            descriptor,
            vec![EmbeddingVector::new("chunk-0", vec![1.0, 1.0])],
        ))],
    );
    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();

    assert_eq!(
        error,
        EmbeddingError::NormalizationMismatch { input_index: 0 }
    );
}

#[tokio::test]
async fn validates_inputs_and_budgets_before_calling_the_provider() {
    let provider = FakeProvider::new(descriptor(2), Vec::new());
    let execution = executor(
        &provider,
        EmbeddingExecutorConfig {
            max_input_text_bytes: 4,
            max_batch_text_bytes: 4,
            max_request_text_bytes: 8,
            ..fast_config()
        },
    );
    let cases = vec![
        (Vec::new(), EmbeddingError::EmptyRequest),
        (
            vec![EmbeddingInput::new("", "text")],
            EmbeddingError::InvalidInput {
                index: 0,
                reason: "identifier is empty, oversized, or contains a control character",
            },
        ),
        (
            vec![input(0, "text"), input(0, "text")],
            EmbeddingError::InvalidInput {
                index: 1,
                reason: "identifier is duplicated",
            },
        ),
        (
            vec![input(0, "oversized")],
            EmbeddingError::BudgetExceeded {
                resource: "input text byte",
                requested: 9,
                limit: 4,
            },
        ),
    ];
    for (inputs, expected) in cases {
        assert_eq!(
            execution
                .embed(inputs, CancellationToken::new())
                .await
                .unwrap_err(),
            expected
        );
    }
    assert_eq!(provider.call_count(), 0);
}

#[tokio::test]
async fn provider_panics_are_converted_to_typed_errors() {
    let provider = FakeProvider::new(descriptor(2), vec![FakeAction::Panic]);
    let error = executor(&provider, fast_config())
        .embed(vec![input(0, "source")], CancellationToken::new())
        .await
        .unwrap_err();

    assert_eq!(
        error,
        EmbeddingError::ProviderPanicked { operation: "embed" }
    );
    assert!(!error.to_string().contains("fake provider panic"));
}

struct PanickingDescriptorProvider;

#[async_trait]
impl EmbeddingProvider for PanickingDescriptorProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        panic!("descriptor panic payload")
    }

    async fn embed(
        &self,
        _request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        unreachable!("constructor must fail before embed")
    }
}

#[test]
fn constructor_contains_descriptor_panics() {
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(PanickingDescriptorProvider);
    let error = EmbeddingExecutor::new(provider, fast_config()).unwrap_err();

    assert_eq!(
        error,
        EmbeddingError::ProviderPanicked {
            operation: "descriptor"
        }
    );
    assert!(!error.to_string().contains("descriptor panic payload"));
}

#[test]
fn debug_and_errors_do_not_expose_source_vector_or_identifier_content() {
    let sentinel = "sk-source-secret-123456";
    let input = EmbeddingInput::new(sentinel, sentinel);
    let request = EmbeddingBatchRequest::new(vec![input.clone()]);
    let vector = EmbeddingVector::new(sentinel, vec![123.456, 789.012]);
    let response = EmbeddingBatchResponse::new(descriptor(2), vec![vector.clone()]);
    let rendered = format!("{input:?} {request:?} {vector:?} {response:?}");

    assert!(!rendered.contains(sentinel));
    assert!(!rendered.contains("123.456"));
    assert!(!EmbeddingProviderError::Authentication
        .to_string()
        .contains(sentinel));
}

#[test]
fn constructor_rejects_invalid_descriptors_and_configs() {
    let provider = FakeProvider::new(descriptor(0), Vec::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider;
    assert_eq!(
        EmbeddingExecutor::new(provider_port, fast_config()).unwrap_err(),
        EmbeddingError::InvalidDescriptor { field: "dimension" }
    );

    let provider = FakeProvider::new(descriptor(2), Vec::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider;
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 0,
        ..fast_config()
    };
    assert_eq!(
        EmbeddingExecutor::new(provider_port, config).unwrap_err(),
        EmbeddingError::InvalidConfiguration {
            field: "max_batch_inputs",
            reason: "must be greater than zero",
        }
    );

    let provider = FakeProvider::new(descriptor(2), Vec::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider;
    let config = EmbeddingExecutorConfig {
        max_retries: 9,
        ..fast_config()
    };
    assert_eq!(
        EmbeddingExecutor::new(provider_port, config).unwrap_err(),
        EmbeddingError::InvalidConfiguration {
            field: "max_retries",
            reason: "must not exceed eight",
        }
    );
}
