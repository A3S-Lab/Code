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
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const MODEL_GENERATION_POOL_MAX_COMPONENT_BYTES: usize = 256;

/// Errors returned while deriving a provider/model capacity pool identity.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelGenerationPoolError {
    #[error("model-generation pool {field} is empty or exceeds {limit} bytes")]
    InvalidComponent { field: &'static str, limit: usize },
    #[error("model-generation pool {field} contains a control character")]
    ControlCharacter { field: &'static str },
    #[error("model-generation pool endpoint is invalid or has no host")]
    InvalidEndpoint,
    #[error("model-generation pool endpoint must not contain credentials")]
    EndpointCredentials,
    #[error("model-generation pool identity is invalid: {0}")]
    InvalidIdentity(String),
    #[error("model-generation pool concurrency must be greater than zero")]
    InvalidConcurrency,
}

/// Digest-only provider/model capacity metadata.
///
/// The pool is intentionally a descriptor, not an executor or semaphore. Its
/// identity binds the non-secret routing facts that share a provider capacity
/// budget; the scheduler or a local admission gate owns the live reservation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelGenerationPool {
    /// Domain-separated digest of provider, model, endpoint origin, and
    /// optional non-secret account scope.
    pub identity: crate::execution_identity::ExecutionIdentityV1,
    /// Maximum active generations for this pool.
    pub max_concurrency: NonZeroUsize,
}

impl ModelGenerationPool {
    /// Build a pool from an already-derived identity.
    pub fn new(
        identity: crate::execution_identity::ExecutionIdentityV1,
        max_concurrency: NonZeroUsize,
    ) -> Result<Self, ModelGenerationPoolError> {
        identity
            .validate()
            .map_err(|error| ModelGenerationPoolError::InvalidIdentity(error.to_string()))?;
        Ok(Self {
            identity,
            max_concurrency,
        })
    }

    /// Derive a stable pool from non-secret provider routing metadata.
    ///
    /// Endpoint paths, queries, fragments, and credentials are deliberately
    /// discarded. Two clients that address the same origin/model therefore
    /// share one capacity key even when their API path configuration differs.
    pub fn for_client(
        provider: &str,
        model: &str,
        endpoint: Option<&str>,
        account_id: Option<&str>,
        concurrency: ModelGenerationConcurrency,
    ) -> Result<Self, ModelGenerationPoolError> {
        let provider = bounded_component("provider", provider)?;
        let model = bounded_component("model", model)?;
        let endpoint_origin = endpoint.map(endpoint_origin).transpose()?;
        let account_id = account_id
            .map(|value| bounded_component("accountId", value))
            .transpose()?;
        let identity = crate::execution_identity::ExecutionIdentityV1::derive(
            crate::execution_identity::MODEL_GENERATION_POOL_IDENTITY_DOMAIN_V1,
            &serde_json::json!({
                "provider": provider,
                "model": model,
                "endpoint_origin": endpoint_origin,
                "account_id": account_id,
            }),
        )
        .map_err(|error| ModelGenerationPoolError::InvalidIdentity(error.to_string()))?;
        Self::new(identity, concurrency.max_concurrency())
    }

    /// Convenience constructor for clients with a concrete endpoint URL.
    pub fn for_endpoint(
        provider: &str,
        model: &str,
        endpoint: &str,
        concurrency: ModelGenerationConcurrency,
    ) -> Result<Self, ModelGenerationPoolError> {
        Self::for_client(provider, model, Some(endpoint), None, concurrency)
    }

    /// Convenience constructor for account-scoped clients.
    pub fn for_account_endpoint(
        provider: &str,
        model: &str,
        endpoint: &str,
        account_id: &str,
        concurrency: ModelGenerationConcurrency,
    ) -> Result<Self, ModelGenerationPoolError> {
        Self::for_client(
            provider,
            model,
            Some(endpoint),
            Some(account_id),
            concurrency,
        )
    }

    pub fn identity(&self) -> &crate::execution_identity::ExecutionIdentityV1 {
        &self.identity
    }

    pub const fn max_concurrency(&self) -> NonZeroUsize {
        self.max_concurrency
    }

    pub fn validate(&self) -> Result<(), ModelGenerationPoolError> {
        self.identity
            .validate()
            .map_err(|error| ModelGenerationPoolError::InvalidIdentity(error.to_string()))?;
        if self.max_concurrency.get() == 0 {
            return Err(ModelGenerationPoolError::InvalidConcurrency);
        }
        Ok(())
    }
}

fn bounded_component(field: &'static str, value: &str) -> Result<String, ModelGenerationPoolError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MODEL_GENERATION_POOL_MAX_COMPONENT_BYTES {
        return Err(ModelGenerationPoolError::InvalidComponent {
            field,
            limit: MODEL_GENERATION_POOL_MAX_COMPONENT_BYTES,
        });
    }
    if value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err(ModelGenerationPoolError::ControlCharacter { field });
    }
    Ok(value.to_string())
}

fn endpoint_origin(value: &str) -> Result<String, ModelGenerationPoolError> {
    let parsed =
        url::Url::parse(value.trim()).map_err(|_| ModelGenerationPoolError::InvalidEndpoint)?;
    if parsed.host_str().is_none() || parsed.scheme().is_empty() {
        return Err(ModelGenerationPoolError::InvalidEndpoint);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ModelGenerationPoolError::EndpointCredentials);
    }
    let origin = parsed.origin().ascii_serialization();
    if origin == "null" {
        return Err(ModelGenerationPoolError::InvalidEndpoint);
    }
    Ok(origin)
}

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
    scheduler: Option<Arc<SchedulerBinding>>,
}

#[derive(Debug)]
struct SchedulerBinding {
    scheduler: Arc<crate::task_scheduler::TaskScheduler>,
    quota: crate::task_scheduler::TaskSchedulerQuota,
    priority: crate::task_scheduler::TaskPriority,
    label: String,
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
            scheduler: None,
        });
        Self { bounded }
    }

    /// Attach a provider/model quota to this gate without creating another
    /// queue. Each permit reserves the quota through the existing scheduler
    /// actor, while the local semaphore retains the client's own contract.
    pub fn with_scheduler_quota(
        self,
        scheduler: Arc<crate::task_scheduler::TaskScheduler>,
        quota: crate::task_scheduler::TaskSchedulerQuota,
        priority: crate::task_scheduler::TaskPriority,
        label: impl Into<String>,
    ) -> Result<Self, crate::task_scheduler::TaskSchedulerError> {
        quota.validate()?;
        let bounded = Arc::new(BoundedAdmission {
            max_concurrency: self.bounded.max_concurrency,
            semaphore: Arc::clone(&self.bounded.semaphore),
            scheduler: Some(Arc::new(SchedulerBinding {
                scheduler,
                quota,
                priority,
                label: label.into(),
            })),
        });
        Ok(Self { bounded })
    }

    /// Copy the scheduler-backed provider reservation from another admission
    /// while retaining this gate's local concurrency. This is used by nested
    /// workflow steps that impose a tighter local limit but must still count
    /// against the session/provider pool in the one shared scheduler actor.
    pub(crate) fn with_scheduler_quota_from(
        self,
        source: &Self,
        label: impl Into<String>,
    ) -> Result<Self, crate::task_scheduler::TaskSchedulerError> {
        let Some(binding) = source.bounded.scheduler.as_ref() else {
            return Ok(self);
        };
        self.with_scheduler_quota(
            Arc::clone(&binding.scheduler),
            binding.quota.clone(),
            binding.priority,
            label,
        )
    }

    pub fn concurrency(&self) -> ModelGenerationConcurrency {
        ModelGenerationConcurrency::bounded(self.bounded.max_concurrency)
    }

    /// Whether every permit also reserves a quota through the shared
    /// scheduler actor.
    pub(crate) fn has_scheduler_quota(&self) -> bool {
        self.bounded.scheduler.is_some()
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
        // Take the local contract first. A scheduler quota-only lease is a
        // scarcer shared resource; acquiring it second prevents a session
        // waiting on its own local semaphore from hoarding provider capacity.
        let scheduler_lease = if let Some(binding) = self.bounded.scheduler.as_ref() {
            Some(
                binding
                    .scheduler
                    .acquire_quota(
                        binding.priority,
                        binding.label.clone(),
                        &binding.quota,
                        cancellation,
                    )
                    .await
                    .map_err(ModelGenerationAdmissionError::from_scheduler_error)?,
            )
        } else {
            None
        };
        Ok(ModelGenerationPermit {
            admission: Arc::clone(&self.bounded),
            _bounded: permit,
            _scheduler: scheduler_lease,
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
    _scheduler: Option<crate::task_scheduler::TaskLease>,
    queue_wait: Duration,
}

impl ModelGenerationPermit {
    pub fn queue_wait(&self) -> Duration {
        self.queue_wait
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelGenerationAdmissionError {
    #[error("model-generation admission cancelled by caller")]
    Cancelled,
    #[error("model-generation admission gate closed")]
    Closed,
    #[error("model-generation permit belongs to a different admission gate")]
    ForeignPermit,
    #[error("scheduler-backed model-generation admission failed: {0}")]
    Scheduler(String),
}

impl ModelGenerationAdmissionError {
    fn from_scheduler_error(error: crate::task_scheduler::TaskSchedulerError) -> Self {
        match error {
            crate::task_scheduler::TaskSchedulerError::Cancelled => Self::Cancelled,
            crate::task_scheduler::TaskSchedulerError::Closed => Self::Closed,
            crate::task_scheduler::TaskSchedulerError::InvalidConfig(message) => {
                Self::Scheduler(message)
            }
        }
    }
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

    #[test]
    fn pool_identity_uses_only_non_secret_endpoint_origin() {
        let concurrency = ModelGenerationConcurrency::bounded(NonZeroUsize::new(2).unwrap());
        let first = ModelGenerationPool::for_client(
            "openai-compatible",
            "model-a",
            Some("https://example.test:443/v1/chat/completions?key=ignored"),
            None,
            concurrency,
        )
        .unwrap();
        let second = ModelGenerationPool::for_client(
            "openai-compatible",
            "model-a",
            Some("https://example.test:443/another-path"),
            None,
            concurrency,
        )
        .unwrap();
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.max_concurrency(), NonZeroUsize::new(2).unwrap());
        let encoded = serde_json::to_string(&first).unwrap();
        assert!(!encoded.contains("ignored"));
        assert!(!encoded.contains("example.test:443/v1"));
    }

    #[test]
    fn pool_identity_separates_provider_model_and_account() {
        let concurrency = ModelGenerationConcurrency::single_flight();
        let base = ModelGenerationPool::for_endpoint(
            "provider-a",
            "model-a",
            "https://example.test",
            concurrency,
        )
        .unwrap();
        let provider = ModelGenerationPool::for_endpoint(
            "provider-b",
            "model-a",
            "https://example.test",
            concurrency,
        )
        .unwrap();
        let model = ModelGenerationPool::for_endpoint(
            "provider-a",
            "model-b",
            "https://example.test",
            concurrency,
        )
        .unwrap();
        let account = ModelGenerationPool::for_account_endpoint(
            "provider-a",
            "model-a",
            "https://example.test",
            "account-1",
            concurrency,
        )
        .unwrap();
        assert_ne!(base.identity, provider.identity);
        assert_ne!(base.identity, model.identity);
        assert_ne!(base.identity, account.identity);
    }

    #[tokio::test]
    async fn tighter_nested_gate_retains_the_session_scheduler_quota() {
        let scheduler = Arc::new(
            crate::task_scheduler::TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
                max_active: 1,
                aging_interval_ms: 1_000,
            })
            .unwrap(),
        );
        let quota =
            crate::task_scheduler::TaskSchedulerQuota::for_scope("provider-session", 1).unwrap();
        let session = ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight())
            .with_scheduler_quota(
                Arc::clone(&scheduler),
                quota,
                crate::task_scheduler::TaskPriority::Foreground,
                "session-generation",
            )
            .unwrap();
        let nested = ModelGenerationAdmission::new(ModelGenerationConcurrency::bounded(
            NonZeroUsize::new(2).unwrap(),
        ))
        .with_scheduler_quota_from(&session, "nested-generation")
        .unwrap();

        let holder = session.acquire(&CancellationToken::new()).await.unwrap();
        let cancellation = CancellationToken::new();
        let waiting = tokio::spawn({
            let nested = nested.clone();
            let cancellation = cancellation.clone();
            async move { nested.acquire(&cancellation).await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(holder);
        let permit = tokio::time::timeout(Duration::from_millis(100), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(permit);
        scheduler.shutdown().await;
    }

    #[test]
    fn pool_rejects_credentials_and_unbounded_components() {
        let concurrency = ModelGenerationConcurrency::single_flight();
        assert!(matches!(
            ModelGenerationPool::for_endpoint(
                "provider",
                "model",
                "https://user:secret@example.test/v1",
                concurrency,
            ),
            Err(ModelGenerationPoolError::EndpointCredentials)
        ));
        assert!(matches!(
            ModelGenerationPool::for_endpoint(
                &"p".repeat(MODEL_GENERATION_POOL_MAX_COMPONENT_BYTES + 1),
                "model",
                "https://example.test",
                concurrency,
            ),
            Err(ModelGenerationPoolError::InvalidComponent {
                field: "provider",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn scheduler_backed_gates_share_provider_capacity_across_sessions() {
        let scheduler = Arc::new(
            crate::task_scheduler::TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
                max_active: 1,
                aging_interval_ms: 60_000,
            })
            .unwrap(),
        );
        let pool = ModelGenerationPool::for_endpoint(
            "provider",
            "model",
            "https://example.test",
            ModelGenerationConcurrency::single_flight(),
        )
        .unwrap();
        let quota = crate::task_scheduler::TaskSchedulerQuota::new(
            pool.identity.clone(),
            pool.max_concurrency().get(),
        )
        .unwrap();
        let admission_a =
            ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight())
                .with_scheduler_quota(
                    Arc::clone(&scheduler),
                    quota.clone(),
                    crate::task_scheduler::TaskPriority::Foreground,
                    "model-generation:session-a",
                )
                .unwrap();
        let admission_b =
            ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight())
                .with_scheduler_quota(
                    Arc::clone(&scheduler),
                    quota.clone(),
                    crate::task_scheduler::TaskPriority::Foreground,
                    "model-generation:session-b",
                )
                .unwrap();
        let first = admission_a
            .acquire(&CancellationToken::new())
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        let waiting = tokio::spawn({
            let admission = admission_b.clone();
            let cancellation = cancellation.clone();
            async move { admission.acquire(&cancellation).await }
        });
        for _ in 0..100 {
            if scheduler.quota_snapshot(&quota).await.unwrap().pending == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(scheduler.quota_snapshot(&quota).await.unwrap().active, 1);
        assert_eq!(scheduler.quota_snapshot(&quota).await.unwrap().pending, 1);
        cancellation.cancel();
        assert!(matches!(
            waiting.await.unwrap(),
            Err(ModelGenerationAdmissionError::Cancelled)
        ));
        drop(first);
        let replacement = tokio::time::timeout(
            Duration::from_millis(100),
            admission_b.acquire(&CancellationToken::new()),
        )
        .await
        .expect("provider quota should be released")
        .unwrap();
        drop(replacement);
        scheduler.shutdown().await;
    }
}
