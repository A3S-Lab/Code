use super::EmbeddingBatchRequest;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub(crate) struct EmbeddingProviderRequestMetrics {
    requests: AtomicUsize,
    inputs: AtomicUsize,
    input_bytes: AtomicUsize,
}

impl EmbeddingProviderRequestMetrics {
    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    pub(crate) fn inputs(&self) -> usize {
        self.inputs.load(Ordering::Relaxed)
    }

    pub(crate) fn input_bytes(&self) -> usize {
        self.input_bytes.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ProviderRequestObservation<'a> {
    Requests(&'a AtomicUsize),
    Detailed(&'a EmbeddingProviderRequestMetrics),
}

impl ProviderRequestObservation<'_> {
    pub(super) fn observe(self, request: &EmbeddingBatchRequest) {
        match self {
            Self::Requests(requests) => {
                requests.fetch_add(1, Ordering::Relaxed);
            }
            Self::Detailed(metrics) => {
                metrics.requests.fetch_add(1, Ordering::Relaxed);
                metrics
                    .inputs
                    .fetch_add(request.inputs().len(), Ordering::Relaxed);
                metrics
                    .input_bytes
                    .fetch_add(request.text_bytes(), Ordering::Relaxed);
            }
        }
    }
}
