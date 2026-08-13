use super::super::*;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::future::pending;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) enum FakeAction {
    Success,
    Reverse,
    Error(EmbeddingProviderError),
    Response(EmbeddingBatchResponse),
    Pending,
    WaitForCancellation,
    Panic,
}

pub(super) struct FakeProvider {
    descriptor: EmbeddingProviderDescriptor,
    actions: Mutex<VecDeque<FakeAction>>,
    calls: Mutex<Vec<Vec<String>>>,
    cancellation_tokens: Mutex<Vec<CancellationToken>>,
    called: Notify,
}

impl FakeProvider {
    pub(super) fn new(
        descriptor: EmbeddingProviderDescriptor,
        actions: Vec<FakeAction>,
    ) -> Arc<Self> {
        Arc::new(Self {
            descriptor,
            actions: Mutex::new(actions.into()),
            calls: Mutex::new(Vec::new()),
            cancellation_tokens: Mutex::new(Vec::new()),
            called: Notify::new(),
        })
    }

    pub(super) fn call_ids(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub(super) fn request_was_cancelled(&self, index: usize) -> bool {
        self.cancellation_tokens
            .lock()
            .unwrap()
            .get(index)
            .is_some_and(CancellationToken::is_cancelled)
    }

    pub(super) async fn wait_for_calls(&self, expected: usize) {
        while self.call_count() < expected {
            self.called.notified().await;
        }
    }

    fn response(&self, request: &EmbeddingBatchRequest, reverse: bool) -> EmbeddingBatchResponse {
        let mut vectors = request
            .inputs()
            .iter()
            .map(|input| {
                let seed = input
                    .id()
                    .bytes()
                    .fold(0u32, |total, value| total.wrapping_add(u32::from(value)));
                EmbeddingVector::new(
                    Arc::<str>::from(input.id()),
                    (0..self.descriptor.dimension)
                        .map(|offset| seed.wrapping_add(offset as u32) as f32)
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        if reverse {
            vectors.reverse();
        }
        EmbeddingBatchResponse::new(self.descriptor.clone(), vectors)
    }
}

#[async_trait]
impl EmbeddingProvider for FakeProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.lock().unwrap().push(
            request
                .inputs()
                .iter()
                .map(|input| input.id().to_owned())
                .collect(),
        );
        self.cancellation_tokens
            .lock()
            .unwrap()
            .push(cancellation.clone());
        self.called.notify_waiters();
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(FakeAction::Success);
        match action {
            FakeAction::Success => Ok(self.response(&request, false)),
            FakeAction::Reverse => Ok(self.response(&request, true)),
            FakeAction::Error(error) => Err(error),
            FakeAction::Response(response) => Ok(response),
            FakeAction::Pending => pending().await,
            FakeAction::WaitForCancellation => {
                cancellation.cancelled().await;
                Err(EmbeddingProviderError::Cancelled)
            }
            FakeAction::Panic => panic!("fake provider panic"),
        }
    }
}

pub(super) fn descriptor(dimension: usize) -> EmbeddingProviderDescriptor {
    EmbeddingProviderDescriptor::new("fake", "deterministic-v1", dimension)
        .with_revision("fixture-1")
}

pub(super) fn input(index: usize, text: &str) -> EmbeddingInput {
    EmbeddingInput::new(format!("chunk-{index}"), text.to_owned())
}

pub(super) fn fast_config() -> EmbeddingExecutorConfig {
    EmbeddingExecutorConfig {
        max_retries: 2,
        base_retry_delay: Duration::ZERO,
        max_retry_delay: Duration::ZERO,
        request_timeout: Duration::from_secs(1),
        ..EmbeddingExecutorConfig::default()
    }
}

pub(super) fn executor(
    provider: &Arc<FakeProvider>,
    config: EmbeddingExecutorConfig,
) -> EmbeddingExecutor {
    let provider: Arc<dyn EmbeddingProvider> = provider.clone();
    EmbeddingExecutor::new(provider, config).unwrap()
}
