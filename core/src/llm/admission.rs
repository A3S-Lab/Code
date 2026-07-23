//! Typed admission control for model-generation transactions.
//!
//! Providers differ in how many generations they can actively serve for one
//! client/account. Callers must not infer that capacity from model names,
//! endpoint URLs, languages, or observed response text. The provider reports a
//! typed concurrency contract, and orchestration code turns it into a shared,
//! cancellation-safe admission gate.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Bounded active model-generation capacity reported by an
/// [`LlmClient`](super::LlmClient).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelGenerationConcurrency {
    max_concurrency: NonZeroUsize,
}

impl ModelGenerationConcurrency {
    /// Conservative contract for providers that do not explicitly advertise
    /// safe parallel generation.
    pub const fn single_flight() -> Self {
        Self {
            max_concurrency: NonZeroUsize::MIN,
        }
    }

    pub const fn bounded(max_concurrency: NonZeroUsize) -> Self {
        Self { max_concurrency }
    }

    pub const fn max_concurrency(self) -> NonZeroUsize {
        self.max_concurrency
    }
}

impl Default for ModelGenerationConcurrency {
    fn default() -> Self {
        Self::single_flight()
    }
}

#[derive(Debug)]
struct BoundedAdmission {
    max_concurrency: NonZeroUsize,
    semaphore: Arc<Semaphore>,
}

/// Shared admission gate derived from a typed provider concurrency contract.
///
/// Clones share the same semaphore. A permit is owned and releases capacity on
/// every exit path, including future cancellation and task abortion.
#[derive(Debug, Clone)]
pub struct ModelGenerationAdmission {
    bounded: Arc<BoundedAdmission>,
}

impl ModelGenerationAdmission {
    pub fn new(concurrency: ModelGenerationConcurrency) -> Self {
        let max_concurrency = concurrency.max_concurrency();
        let bounded = Arc::new(BoundedAdmission {
            max_concurrency,
            semaphore: Arc::new(Semaphore::new(max_concurrency.get())),
        });
        Self { bounded }
    }

    pub fn concurrency(&self) -> ModelGenerationConcurrency {
        ModelGenerationConcurrency::bounded(self.bounded.max_concurrency)
    }

    /// Wait for active-generation capacity without applying an active
    /// generation deadline to the queue wait.
    pub async fn acquire(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<ModelGenerationPermit, ModelGenerationAdmissionError> {
        let queued_at = Instant::now();
        let acquire = Arc::clone(&self.bounded.semaphore).acquire_owned();
        tokio::pin!(acquire);
        let permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(ModelGenerationAdmissionError::Cancelled);
            }
            permit = &mut acquire => permit.map_err(|_| {
                ModelGenerationAdmissionError::Closed
            })?,
        };
        Ok(ModelGenerationPermit {
            admission: Arc::clone(&self.bounded),
            _bounded: permit,
            queue_wait: queued_at.elapsed(),
        })
    }

    pub(crate) fn owns(&self, permit: &ModelGenerationPermit) -> bool {
        Arc::ptr_eq(&self.bounded, &permit.admission)
    }

    #[cfg(test)]
    fn available_permits(&self) -> usize {
        self.bounded.semaphore.available_permits()
    }
}

impl Default for ModelGenerationAdmission {
    fn default() -> Self {
        Self::new(ModelGenerationConcurrency::default())
    }
}

/// Owned capacity for one active model-generation transaction.
#[derive(Debug)]
#[must_use = "dropping the permit releases model-generation capacity"]
pub struct ModelGenerationPermit {
    admission: Arc<BoundedAdmission>,
    _bounded: OwnedSemaphorePermit,
    queue_wait: Duration,
}

impl ModelGenerationPermit {
    pub fn queue_wait(&self) -> Duration {
        self.queue_wait
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModelGenerationAdmissionError {
    #[error("model-generation admission cancelled by caller")]
    Cancelled,
    #[error("model-generation admission gate closed")]
    Closed,
    #[error("model-generation permit belongs to a different admission gate")]
    ForeignPermit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelling_a_queued_waiter_does_not_consume_capacity() {
        let admission = ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight());
        let holder = admission
            .acquire(&CancellationToken::new())
            .await
            .expect("first permit");
        assert_eq!(admission.available_permits(), 0);

        let cancellation = CancellationToken::new();
        let waiter = tokio::spawn({
            let admission = admission.clone();
            let cancellation = cancellation.clone();
            async move { admission.acquire(&cancellation).await }
        });
        tokio::task::yield_now().await;
        cancellation.cancel();
        assert!(matches!(
            waiter.await.expect("waiter join"),
            Err(ModelGenerationAdmissionError::Cancelled)
        ));

        drop(holder);
        let replacement = tokio::time::timeout(
            Duration::from_millis(100),
            admission.acquire(&CancellationToken::new()),
        )
        .await
        .expect("cancelled waiter must release its queue position")
        .expect("replacement permit");
        drop(replacement);
    }
}
