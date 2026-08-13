use super::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingError, EmbeddingExecution,
    EmbeddingFailureKind, EmbeddingInput, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingResult, EmbeddingVector,
};
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MAX_DESCRIPTOR_TEXT_BYTES: usize = 256;
const MAX_INPUT_ID_BYTES: usize = 512;
const MAX_EMBEDDING_DIMENSION: usize = 65_536;
const MAX_RETRIES: u32 = 8;
const MAX_OPERATION_DURATION: Duration = Duration::from_secs(5 * 60);
const UNIT_NORM_TOLERANCE: f64 = 0.01;

/// Hard limits and retry policy for one embedding executor generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddingExecutorConfig {
    pub max_batch_inputs: usize,
    pub max_batch_text_bytes: usize,
    pub max_input_text_bytes: usize,
    pub max_request_inputs: usize,
    pub max_request_text_bytes: usize,
    pub max_batch_vector_bytes: usize,
    pub max_request_vector_bytes: usize,
    pub max_retries: u32,
    pub base_retry_delay: Duration,
    pub max_retry_delay: Duration,
    pub request_timeout: Duration,
}

impl Default for EmbeddingExecutorConfig {
    fn default() -> Self {
        Self {
            max_batch_inputs: 64,
            max_batch_text_bytes: 256 * 1024,
            max_input_text_bytes: 64 * 1024,
            max_request_inputs: 4_096,
            max_request_text_bytes: 16 * 1024 * 1024,
            max_batch_vector_bytes: 32 * 1024 * 1024,
            max_request_vector_bytes: 64 * 1024 * 1024,
            max_retries: 2,
            base_retry_delay: Duration::from_millis(100),
            max_retry_delay: Duration::from_secs(2),
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl EmbeddingExecutorConfig {
    fn validate(self) -> EmbeddingResult<Self> {
        for (field, value) in [
            ("max_batch_inputs", self.max_batch_inputs),
            ("max_batch_text_bytes", self.max_batch_text_bytes),
            ("max_input_text_bytes", self.max_input_text_bytes),
            ("max_request_inputs", self.max_request_inputs),
            ("max_request_text_bytes", self.max_request_text_bytes),
            ("max_batch_vector_bytes", self.max_batch_vector_bytes),
            ("max_request_vector_bytes", self.max_request_vector_bytes),
        ] {
            if value == 0 {
                return Err(EmbeddingError::InvalidConfiguration {
                    field,
                    reason: "must be greater than zero",
                });
            }
        }
        if self.max_batch_inputs > self.max_request_inputs {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "max_batch_inputs",
                reason: "must not exceed max_request_inputs",
            });
        }
        if self.max_input_text_bytes > self.max_batch_text_bytes
            || self.max_batch_text_bytes > self.max_request_text_bytes
        {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "text byte limits",
                reason: "must be monotonic from input to batch to request",
            });
        }
        if self.max_batch_vector_bytes > self.max_request_vector_bytes {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "max_batch_vector_bytes",
                reason: "must not exceed max_request_vector_bytes",
            });
        }
        if self.request_timeout.is_zero() {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "request_timeout",
                reason: "must be greater than zero",
            });
        }
        if self.request_timeout > MAX_OPERATION_DURATION {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "request_timeout",
                reason: "must not exceed five minutes",
            });
        }
        if self.max_retries > MAX_RETRIES {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "max_retries",
                reason: "must not exceed eight",
            });
        }
        if self.max_retry_delay > MAX_OPERATION_DURATION {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "max_retry_delay",
                reason: "must not exceed five minutes",
            });
        }
        if self.base_retry_delay > self.max_retry_delay {
            return Err(EmbeddingError::InvalidConfiguration {
                field: "base_retry_delay",
                reason: "must not exceed max_retry_delay",
            });
        }
        Ok(self)
    }
}

/// Validating, batching wrapper around one host-injected provider generation.
#[derive(Clone)]
pub struct EmbeddingExecutor {
    provider: Arc<dyn EmbeddingProvider>,
    descriptor: EmbeddingProviderDescriptor,
    config: EmbeddingExecutorConfig,
}

impl std::fmt::Debug for EmbeddingExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddingExecutor")
            .field("descriptor", &self.descriptor)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EmbeddingExecutor {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        config: EmbeddingExecutorConfig,
    ) -> EmbeddingResult<Self> {
        let descriptor = std::panic::catch_unwind(AssertUnwindSafe(|| provider.descriptor()))
            .map_err(|_| EmbeddingError::ProviderPanicked {
                operation: "descriptor",
            })?;
        validate_descriptor(&descriptor)?;
        Ok(Self {
            provider,
            descriptor,
            config: config.validate()?,
        })
    }

    pub fn descriptor(&self) -> &EmbeddingProviderDescriptor {
        &self.descriptor
    }

    pub fn config(&self) -> EmbeddingExecutorConfig {
        self.config
    }

    /// Embed all inputs atomically from the caller's perspective.
    ///
    /// No partial vector list is returned if a later batch fails validation.
    pub async fn embed(
        &self,
        inputs: Vec<EmbeddingInput>,
        cancellation: CancellationToken,
    ) -> EmbeddingResult<EmbeddingExecution> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingError::Cancelled);
        }
        let batches = plan_batches(&inputs, self.descriptor.dimension, self.config)?;
        let mut vectors = Vec::with_capacity(inputs.len());
        let mut provider_attempts = 0usize;
        for range in &batches {
            let request = EmbeddingBatchRequest::new(inputs[range.clone()].to_vec());
            let (response, attempts) = self.call_batch(request.clone(), &cancellation).await?;
            provider_attempts = provider_attempts.saturating_add(attempts);
            vectors.extend(validate_response(
                &self.descriptor,
                request.inputs(),
                response,
            )?);
        }
        Ok(EmbeddingExecution {
            descriptor: self.descriptor.clone(),
            vectors,
            batch_count: batches.len(),
            provider_attempts,
        })
    }

    async fn call_batch(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: &CancellationToken,
    ) -> EmbeddingResult<(EmbeddingBatchResponse, usize)> {
        for attempt in 0..=self.config.max_retries {
            if cancellation.is_cancelled() {
                return Err(EmbeddingError::Cancelled);
            }
            let attempt_token = cancellation.child_token();
            let provider_call =
                AssertUnwindSafe(self.provider.embed(request.clone(), attempt_token.clone()))
                    .catch_unwind();
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    attempt_token.cancel();
                    return Err(EmbeddingError::Cancelled);
                }
                result = tokio::time::timeout(
                    self.config.request_timeout,
                    provider_call,
                ) => match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => {
                        return Err(EmbeddingError::ProviderPanicked { operation: "embed" });
                    }
                    Err(_) => {
                        attempt_token.cancel();
                        Err(EmbeddingProviderError::Timeout)
                    }
                }
            };
            let attempts = attempt as usize + 1;
            match result {
                Ok(response) => return Ok((response, attempts)),
                Err(EmbeddingProviderError::Cancelled) => return Err(EmbeddingError::Cancelled),
                Err(error) if error.is_retryable() && attempt < self.config.max_retries => {
                    let delay = retry_delay(&error, attempt, self.config);
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                Err(error) if error.is_retryable() => {
                    return Err(EmbeddingError::RetriesExhausted {
                        kind: error.kind(),
                        attempts,
                    })
                }
                Err(error) => {
                    return Err(EmbeddingError::ProviderFailure {
                        kind: error.kind(),
                        attempts,
                    })
                }
            }
        }
        Err(EmbeddingError::RetriesExhausted {
            kind: EmbeddingFailureKind::Other,
            attempts: self.config.max_retries as usize + 1,
        })
    }
}

fn validate_descriptor(descriptor: &EmbeddingProviderDescriptor) -> EmbeddingResult<()> {
    for (field, value) in [
        ("provider", descriptor.provider.as_str()),
        ("model", descriptor.model.as_str()),
    ] {
        if value.trim().is_empty()
            || value.len() > MAX_DESCRIPTOR_TEXT_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(EmbeddingError::InvalidDescriptor { field });
        }
    }
    if descriptor.revision.as_ref().is_some_and(|revision| {
        revision.trim().is_empty()
            || revision.len() > MAX_DESCRIPTOR_TEXT_BYTES
            || revision.chars().any(char::is_control)
    }) {
        return Err(EmbeddingError::InvalidDescriptor { field: "revision" });
    }
    if descriptor.dimension == 0 || descriptor.dimension > MAX_EMBEDDING_DIMENSION {
        return Err(EmbeddingError::InvalidDescriptor { field: "dimension" });
    }
    Ok(())
}

fn plan_batches(
    inputs: &[EmbeddingInput],
    dimension: usize,
    config: EmbeddingExecutorConfig,
) -> EmbeddingResult<Vec<Range<usize>>> {
    if inputs.is_empty() {
        return Err(EmbeddingError::EmptyRequest);
    }
    if inputs.len() > config.max_request_inputs {
        return Err(EmbeddingError::BudgetExceeded {
            resource: "request input count",
            requested: inputs.len(),
            limit: config.max_request_inputs,
        });
    }
    let mut seen = HashSet::with_capacity(inputs.len());
    let mut total_text_bytes = 0usize;
    for (index, input) in inputs.iter().enumerate() {
        if input.id().is_empty()
            || input.id().len() > MAX_INPUT_ID_BYTES
            || input.id().chars().any(char::is_control)
        {
            return Err(EmbeddingError::InvalidInput {
                index,
                reason: "identifier is empty, oversized, or contains a control character",
            });
        }
        if !seen.insert(input.id()) {
            return Err(EmbeddingError::InvalidInput {
                index,
                reason: "identifier is duplicated",
            });
        }
        if input.text().is_empty() {
            return Err(EmbeddingError::InvalidInput {
                index,
                reason: "text must not be empty",
            });
        }
        if input.text_bytes() > config.max_input_text_bytes {
            return Err(EmbeddingError::BudgetExceeded {
                resource: "input text byte",
                requested: input.text_bytes(),
                limit: config.max_input_text_bytes,
            });
        }
        total_text_bytes = total_text_bytes.saturating_add(input.text_bytes());
    }
    if total_text_bytes > config.max_request_text_bytes {
        return Err(EmbeddingError::BudgetExceeded {
            resource: "request text byte",
            requested: total_text_bytes,
            limit: config.max_request_text_bytes,
        });
    }
    let vector_bytes_per_input = dimension.saturating_mul(std::mem::size_of::<f32>());
    let request_vector_bytes = vector_bytes_per_input.saturating_mul(inputs.len());
    if request_vector_bytes > config.max_request_vector_bytes {
        return Err(EmbeddingError::BudgetExceeded {
            resource: "request vector byte",
            requested: request_vector_bytes,
            limit: config.max_request_vector_bytes,
        });
    }

    let mut batches = Vec::new();
    let mut start = 0usize;
    let mut batch_bytes = 0usize;
    for (index, input) in inputs.iter().enumerate() {
        let would_exceed_items = index.saturating_sub(start) >= config.max_batch_inputs;
        let would_exceed_bytes =
            batch_bytes.saturating_add(input.text_bytes()) > config.max_batch_text_bytes;
        let batch_items = index.saturating_sub(start).saturating_add(1);
        let would_exceed_vector_bytes =
            vector_bytes_per_input.saturating_mul(batch_items) > config.max_batch_vector_bytes;
        if index > start && (would_exceed_items || would_exceed_bytes || would_exceed_vector_bytes)
        {
            batches.push(start..index);
            start = index;
            batch_bytes = 0;
        }
        if vector_bytes_per_input > config.max_batch_vector_bytes {
            return Err(EmbeddingError::BudgetExceeded {
                resource: "batch vector byte",
                requested: vector_bytes_per_input,
                limit: config.max_batch_vector_bytes,
            });
        }
        batch_bytes = batch_bytes.saturating_add(input.text_bytes());
    }
    if start < inputs.len() {
        batches.push(start..inputs.len());
    }
    Ok(batches)
}

fn retry_delay(
    error: &EmbeddingProviderError,
    attempt: u32,
    config: EmbeddingExecutorConfig,
) -> Duration {
    error
        .retry_after()
        .unwrap_or_else(|| {
            config
                .base_retry_delay
                .saturating_mul(1u32 << attempt.min(16))
        })
        .min(config.max_retry_delay)
}

fn validate_response(
    expected_descriptor: &EmbeddingProviderDescriptor,
    inputs: &[EmbeddingInput],
    response: EmbeddingBatchResponse,
) -> EmbeddingResult<Vec<EmbeddingVector>> {
    validate_descriptor(&response.descriptor)?;
    if response.descriptor != *expected_descriptor {
        return Err(EmbeddingError::DescriptorChanged);
    }
    if response.vectors.len() != inputs.len() {
        return Err(EmbeddingError::OutputCountMismatch {
            expected: inputs.len(),
            actual: response.vectors.len(),
        });
    }
    let indices = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.id(), index))
        .collect::<HashMap<_, _>>();
    let mut ordered = vec![None; inputs.len()];
    for vector in response.vectors {
        let Some(&input_index) = indices.get(vector.id.as_ref()) else {
            return Err(EmbeddingError::UnexpectedOutput);
        };
        if ordered[input_index].is_some() {
            return Err(EmbeddingError::DuplicateOutput { input_index });
        }
        if vector.values.len() != expected_descriptor.dimension {
            return Err(EmbeddingError::DimensionMismatch {
                input_index,
                expected: expected_descriptor.dimension,
                actual: vector.values.len(),
            });
        }
        if let Some(position) = vector.values.iter().position(|value| !value.is_finite()) {
            return Err(EmbeddingError::NonFiniteValue {
                input_index,
                position,
            });
        }
        if expected_descriptor.normalization == EmbeddingNormalization::Unit {
            let norm = vector
                .values
                .iter()
                .map(|value| f64::from(*value).powi(2))
                .sum::<f64>()
                .sqrt();
            if (norm - 1.0).abs() > UNIT_NORM_TOLERANCE {
                return Err(EmbeddingError::NormalizationMismatch { input_index });
            }
        }
        ordered[input_index] = Some(EmbeddingVector::new(
            Arc::<str>::from(inputs[input_index].id()),
            vector.values,
        ));
    }
    Ok(ordered.into_iter().flatten().collect())
}
