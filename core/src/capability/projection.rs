use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::time::Instant;

use crate::cognitive_context::CognitiveContextSession;
use crate::commands::SlashCommand;
use crate::context::ContextProvider;
use crate::dynamic_workflow::DynamicWorkflowRuntime;
use crate::hooks::HookBinding;
use crate::skills::Skill;
use crate::subagent::AgentDefinition;
use crate::tools::Tool;

use super::{
    CapabilityEffect, CapabilityId, CapabilityKind, CapabilityProjectionError,
    CapabilityReadinessPlan, CapabilitySet, CapabilityValue, CodeCatalogGeneration, McpBinding,
    ScopeClosePolicy, Sha256Digest, UseGenerationLeaseProvider,
};

/// Immutable pairing of one identity set with exactly one typed runtime value
/// for every descriptor.
#[derive(Debug)]
pub struct CapabilityProjection {
    set: Arc<CapabilitySet>,
    readiness: Arc<CapabilityReadinessPlan>,
    values: BTreeMap<CapabilityId, CapabilityValue>,
}

impl CapabilityProjection {
    pub fn new(
        set: Arc<CapabilitySet>,
        values: impl IntoIterator<Item = (CapabilityId, CapabilityValue)>,
    ) -> Result<Arc<Self>, CapabilityProjectionError> {
        let readiness = Arc::new(CapabilityReadinessPlan::from_set(&set)?);
        Self::with_readiness(set, readiness, values)
    }

    pub(super) fn with_readiness(
        set: Arc<CapabilitySet>,
        readiness: Arc<CapabilityReadinessPlan>,
        values: impl IntoIterator<Item = (CapabilityId, CapabilityValue)>,
    ) -> Result<Arc<Self>, CapabilityProjectionError> {
        if !readiness.matches(&set) {
            return Err(CapabilityProjectionError::ReadinessPlanMismatch {
                expected_generation: set.generation().get(),
                actual_generation: readiness.generation().get(),
                digest_mismatch: readiness.digest() != set.digest(),
            });
        }
        let mut canonical = BTreeMap::new();
        for (id, value) in values {
            if canonical.insert(id.clone(), value).is_some() {
                return Err(CapabilityProjectionError::DuplicateValue {
                    capability: id.to_string(),
                });
            }
        }

        for (id, descriptor) in set.iter() {
            if descriptor.id().kind() == CapabilityKind::Ui {
                return Err(CapabilityProjectionError::UnsupportedKind {
                    kind: CapabilityKind::Ui,
                });
            }
            let Some(value) = canonical.get(id) else {
                return Err(CapabilityProjectionError::MissingValue {
                    capability: id.to_string(),
                });
            };
            if value.kind() != descriptor.id().kind() {
                return Err(CapabilityProjectionError::KindMismatch {
                    capability: id.to_string(),
                    descriptor_kind: descriptor.id().kind(),
                    value_kind: value.kind(),
                });
            }
            if let Some(actual) = value.public_name() {
                if actual != descriptor.public_name() {
                    return Err(CapabilityProjectionError::PublicNameMismatch {
                        capability: id.to_string(),
                        expected: descriptor.public_name().to_owned(),
                        actual: actual.to_owned(),
                    });
                }
            }
        }
        for id in canonical.keys() {
            if !set.contains(id) {
                return Err(CapabilityProjectionError::UnexpectedValue {
                    capability: id.to_string(),
                });
            }
        }

        Ok(Arc::new(Self {
            set,
            readiness,
            values: canonical,
        }))
    }

    pub fn set(&self) -> &CapabilitySet {
        &self.set
    }

    pub(crate) fn set_arc(&self) -> &Arc<CapabilitySet> {
        &self.set
    }

    pub fn readiness_plan(&self) -> &CapabilityReadinessPlan {
        &self.readiness
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.values.contains_key(id)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&CapabilityId, &CapabilityValue)> {
        self.values.iter()
    }

    pub fn tool(&self, id: &CapabilityId) -> Option<&dyn Tool> {
        match self.values.get(id) {
            Some(CapabilityValue::Tool(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn skill(&self, id: &CapabilityId) -> Option<&Skill> {
        match self.values.get(id) {
            Some(CapabilityValue::Skill(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn agent(&self, id: &CapabilityId) -> Option<&AgentDefinition> {
        match self.values.get(id) {
            Some(CapabilityValue::Agent(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn command(&self, id: &CapabilityId) -> Option<&dyn SlashCommand> {
        match self.values.get(id) {
            Some(CapabilityValue::Command(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn hook(&self, id: &CapabilityId) -> Option<&HookBinding> {
        match self.values.get(id) {
            Some(CapabilityValue::Hook(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn mcp(&self, id: &CapabilityId) -> Option<&McpBinding> {
        match self.values.get(id) {
            Some(CapabilityValue::Mcp(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn flow(&self, id: &CapabilityId) -> Option<&DynamicWorkflowRuntime> {
        match self.values.get(id) {
            Some(CapabilityValue::Flow(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn knowledge(&self, id: &CapabilityId) -> Option<&CognitiveContextSession> {
        match self.values.get(id) {
            Some(CapabilityValue::Knowledge(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn context(&self, id: &CapabilityId) -> Option<&dyn ContextProvider> {
        match self.values.get(id) {
            Some(CapabilityValue::Context(value)) => Some(value.as_ref()),
            _ => None,
        }
    }
}

/// Exact local generation and identity digest used by catalog CAS publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityCatalogStamp {
    generation: CodeCatalogGeneration,
    digest: Sha256Digest,
}

impl CapabilityCatalogStamp {
    fn from_projection(projection: &CapabilityProjection) -> Self {
        Self {
            generation: projection.set().generation(),
            digest: projection.set().digest().clone(),
        }
    }

    pub const fn generation(&self) -> CodeCatalogGeneration {
        self.generation
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

/// Successful all-or-nothing projection publication evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityCommitReceipt {
    previous: CapabilityCatalogStamp,
    committed: CapabilityCatalogStamp,
}

impl CapabilityCommitReceipt {
    pub fn previous(&self) -> &CapabilityCatalogStamp {
        &self.previous
    }

    pub fn committed(&self) -> &CapabilityCatalogStamp {
        &self.committed
    }
}

#[derive(Clone, Copy)]
enum CleanupReason {
    Rollback,
    Retired,
}

struct CleanupBatch {
    reason: CleanupReason,
    effects: Vec<Box<dyn CapabilityEffect>>,
}

#[derive(Default)]
struct CleanupQueue {
    batches: Mutex<VecDeque<CleanupBatch>>,
}

impl CleanupQueue {
    fn enqueue(&self, reason: CleanupReason, effects: Vec<Box<dyn CapabilityEffect>>) {
        if effects.is_empty() {
            return;
        }
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(CleanupBatch { reason, effects });
    }

    fn take_all(&self) -> VecDeque<CleanupBatch> {
        std::mem::take(
            &mut *self
                .batches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn len(&self) -> usize {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

struct PublishedGeneration {
    projection: Arc<CapabilityProjection>,
    stamp: CapabilityCatalogStamp,
    use_lease_provider: Option<Arc<dyn UseGenerationLeaseProvider>>,
    effects: Mutex<Vec<Box<dyn CapabilityEffect>>>,
    cleanup: Arc<CleanupQueue>,
}

impl PublishedGeneration {
    fn new(
        projection: Arc<CapabilityProjection>,
        use_lease_provider: Option<Arc<dyn UseGenerationLeaseProvider>>,
        effects: Vec<Box<dyn CapabilityEffect>>,
        cleanup: Arc<CleanupQueue>,
    ) -> Self {
        let stamp = CapabilityCatalogStamp::from_projection(&projection);
        Self {
            projection,
            stamp,
            use_lease_provider,
            effects: Mutex::new(effects),
            cleanup,
        }
    }
}

impl Drop for PublishedGeneration {
    fn drop(&mut self) {
        let effects = std::mem::take(
            self.effects
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.cleanup.enqueue(CleanupReason::Retired, effects);
    }
}

struct CatalogState {
    current: Arc<PublishedGeneration>,
}

pub(super) struct CatalogInner {
    state: Mutex<CatalogState>,
    cleanup: Arc<CleanupQueue>,
}

impl CatalogInner {
    pub(super) fn current_stamp(&self) -> CapabilityCatalogStamp {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .current
            .stamp
            .clone()
    }

    pub(super) fn enqueue_rollback(&self, effects: Vec<Box<dyn CapabilityEffect>>) {
        self.cleanup.enqueue(CleanupReason::Rollback, effects);
    }

    pub(super) fn publish(
        &self,
        base: &CapabilityCatalogStamp,
        projection: Arc<CapabilityProjection>,
        use_lease_provider: Option<Arc<dyn UseGenerationLeaseProvider>>,
        effects: Vec<Box<dyn CapabilityEffect>>,
    ) -> Result<CapabilityCommitReceipt, CapabilityProjectionError> {
        let committed = CapabilityCatalogStamp::from_projection(&projection);
        let old = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let actual = &state.current.stamp;
            if actual != base {
                let error = CapabilityProjectionError::CommitConflict {
                    expected_generation: base.generation().get(),
                    expected_digest: base.digest().to_string(),
                    actual_generation: actual.generation().get(),
                    actual_digest: actual.digest().to_string(),
                };
                drop(state);
                self.enqueue_rollback(effects);
                return Err(error);
            }
            let published = Arc::new(PublishedGeneration::new(
                projection,
                use_lease_provider,
                effects,
                Arc::clone(&self.cleanup),
            ));
            std::mem::replace(&mut state.current, published)
        };
        let previous = old.stamp.clone();
        drop(old);
        Ok(CapabilityCommitReceipt {
            previous,
            committed,
        })
    }
}

/// Session-local immutable capability publication catalog.
///
/// Readers only clone one `Arc` under a short mutex and then resolve through a
/// pinned [`CapabilityProjectionLease`]. Writers use an exact generation and
/// digest compare-and-swap; a losing writer cannot mutate the current value.
pub struct CapabilityCatalog {
    pub(super) inner: Arc<CatalogInner>,
}

impl CapabilityCatalog {
    pub fn new(initial: Arc<CapabilityProjection>) -> Self {
        let cleanup = Arc::new(CleanupQueue::default());
        let current = Arc::new(PublishedGeneration::new(
            initial,
            None,
            Vec::new(),
            Arc::clone(&cleanup),
        ));
        Self {
            inner: Arc::new(CatalogInner {
                state: Mutex::new(CatalogState { current }),
                cleanup,
            }),
        }
    }

    pub fn current_stamp(&self) -> CapabilityCatalogStamp {
        self.inner.current_stamp()
    }

    pub fn pin(&self) -> CapabilityProjectionLease {
        let generation = Arc::clone(
            &self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .current,
        );
        CapabilityProjectionLease { generation }
    }

    pub fn pending_cleanup_batches(&self) -> usize {
        self.inner.cleanup.len()
    }

    pub(crate) fn retire_current_effects(&self) {
        let generation = Arc::clone(
            &self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .current,
        );
        let effects = std::mem::take(
            &mut *generation
                .effects
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.inner.cleanup.enqueue(CleanupReason::Retired, effects);
    }

    pub async fn drain_cleanup(&self) -> CapabilityCleanupReport {
        self.drain_cleanup_with_policy(ScopeClosePolicy::default())
            .await
    }

    pub async fn drain_cleanup_with_policy(
        &self,
        policy: ScopeClosePolicy,
    ) -> CapabilityCleanupReport {
        let mut batches = self.inner.cleanup.take_all();
        let deadline = Instant::now() + policy.timeout();
        let mut report = CapabilityCleanupReport::default();

        while let Some(mut batch) = batches.pop_front() {
            match batch.reason {
                CleanupReason::Rollback => report.rollback_batches += 1,
                CleanupReason::Retired => report.retired_batches += 1,
            }
            while let Some(effect) = batch.effects.pop() {
                if Instant::now() >= deadline {
                    report.effects_timed_out += 1 + batch.effects.len();
                    break;
                }
                match tokio::time::timeout_at(deadline, effect.close()).await {
                    Ok(Ok(())) => report.effects_closed += 1,
                    Ok(Err(_)) => report.effects_failed += 1,
                    Err(_) => report.effects_timed_out += 1,
                }
            }
        }
        report
    }
}

impl fmt::Debug for CapabilityCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityCatalog")
            .field("current", &self.current_stamp())
            .field("pending_cleanup_batches", &self.pending_cleanup_batches())
            .finish()
    }
}

/// Non-clone reader lease retaining one exact projected generation.
///
/// The contained values are exposed by borrow, so ordinary execution cannot
/// accidentally switch to the catalog's latest generation. When the last
/// lease and catalog pointer to a retired generation disappear, Rust `Arc`
/// ownership moves its effects to the asynchronous cleanup queue.
#[must_use = "a projection lease pins one exact catalog generation"]
pub struct CapabilityProjectionLease {
    generation: Arc<PublishedGeneration>,
}

impl CapabilityProjectionLease {
    pub fn stamp(&self) -> &CapabilityCatalogStamp {
        &self.generation.stamp
    }

    pub fn projection(&self) -> &CapabilityProjection {
        &self.generation.projection
    }

    pub(super) fn use_lease_provider(&self) -> Option<&Arc<dyn UseGenerationLeaseProvider>> {
        self.generation.use_lease_provider.as_ref()
    }
}

impl fmt::Debug for CapabilityProjectionLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityProjectionLease")
            .field("stamp", self.stamp())
            .finish_non_exhaustive()
    }
}

/// Bounded reverse-teardown result for retired or rolled-back effects.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityCleanupReport {
    pub rollback_batches: usize,
    pub retired_batches: usize,
    pub effects_closed: usize,
    pub effects_failed: usize,
    pub effects_timed_out: usize,
}

impl CapabilityCleanupReport {
    pub const fn is_clean(&self) -> bool {
        self.effects_failed == 0 && self.effects_timed_out == 0
    }
}
