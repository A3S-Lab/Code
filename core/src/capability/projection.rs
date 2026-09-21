use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::time::Instant;

use crate::cognitive_context::CognitiveContextSession;
use crate::commands::SlashCommand;
use crate::context::ContextProvider;
use crate::hooks::HookBinding;
use crate::skills::Skill;
use crate::subagent::AgentDefinition;
use crate::tools::Tool;

use super::{
    CapabilityEffect, CapabilityId, CapabilityKind, CapabilityProjectionError,
    CapabilityReadinessPlan, CapabilitySet, CapabilityValue, CodeCatalogGeneration,
    KnowledgeSurfaceBinding, McpBinding, ScopeClosePolicy, Sha256Digest, UiBinding,
    UseGenerationLeaseProvider,
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
            if value
                .public_name()
                .is_some_and(|actual| actual != descriptor.public_name())
            {
                return Err(CapabilityProjectionError::PublicNameMismatch {
                    capability: id.to_string(),
                    expected: descriptor.public_name().to_owned(),
                    actual: value
                        .public_name()
                        .expect("is_some_and already observed a public name")
                        .to_owned(),
                });
            }
            if let CapabilityValue::Ui(binding) = value {
                if descriptor.surface_digest() != binding.surface_digest() {
                    return Err(CapabilityProjectionError::SurfaceDigestMismatch {
                        capability: id.to_string(),
                        expected: descriptor.surface_digest().to_string(),
                        actual: binding.surface_digest().to_string(),
                    });
                }
                for dependency in descriptor.dependencies() {
                    if !matches!(
                        dependency.kind(),
                        CapabilityKind::Tool
                            | CapabilityKind::Skill
                            | CapabilityKind::Mcp
                            | CapabilityKind::Flow
                    ) {
                        return Err(CapabilityProjectionError::UnsupportedUiDependencyKind {
                            capability: id.to_string(),
                            dependency: dependency.to_string(),
                            dependency_kind: dependency.kind(),
                        });
                    }
                }
            }
            if let CapabilityValue::KnowledgeSurface(binding) = value {
                if descriptor.surface_digest() != binding.surface_digest() {
                    return Err(CapabilityProjectionError::SurfaceDigestMismatch {
                        capability: id.to_string(),
                        expected: descriptor.surface_digest().to_string(),
                        actual: binding.surface_digest().to_string(),
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

    #[cfg(feature = "dynamic-workflow")]
    pub fn flow(&self, id: &CapabilityId) -> Option<&crate::capability::FlowBinding> {
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

    pub fn knowledge_surface(&self, id: &CapabilityId) -> Option<&KnowledgeSurfaceBinding> {
        match self.values.get(id) {
            Some(CapabilityValue::KnowledgeSurface(value)) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn ui(&self, id: &CapabilityId) -> Option<&UiBinding> {
        match self.values.get(id) {
            Some(CapabilityValue::Ui(value)) => Some(value.as_ref()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilitySource, Sha256Digest};
    use crate::cognitive_context::CognitiveContextProvider;
    use crate::hooks::HookHandler;
    use crate::skills::{Skill, SkillKind};

    fn digest(byte: char) -> Sha256Digest {
        assert!(
            byte.is_ascii_hexdigit(),
            "test digests must use hex characters"
        );
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
    }

    fn sample_skill_value(name: &str) -> CapabilityValue {
        CapabilityValue::Skill(Arc::new(Skill {
            name: name.to_owned(),
            description: "coverage".to_owned(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "body".to_owned(),
            tags: vec![],
            version: None,
        }))
    }

    #[test]
    fn empty_projection_exposes_accessors_and_none_lookups() {
        let set = CapabilitySet::empty().unwrap();
        let projection = CapabilityProjection::new(Arc::clone(&set), []).unwrap();
        assert!(projection.is_empty());
        assert_eq!(projection.len(), 0);
        assert_eq!(projection.iter().len(), 0);
        assert!(Arc::ptr_eq(projection.set_arc(), &set));
        assert_eq!(projection.set().generation().get(), set.generation().get());
        assert_eq!(
            projection.readiness_plan().generation().get(),
            set.generation().get()
        );

        let source = CapabilitySource::builtin("a3s-code", digest('a')).unwrap();
        let missing = CapabilityId::new(&source, CapabilityKind::Skill, "missing").unwrap();
        assert!(!projection.contains(&missing));
        assert!(projection.tool(&missing).is_none());
        assert!(projection.skill(&missing).is_none());
        assert!(projection.agent(&missing).is_none());
        assert!(projection.command(&missing).is_none());
        assert!(projection.hook(&missing).is_none());
        assert!(projection.mcp(&missing).is_none());
        assert!(projection.knowledge(&missing).is_none());
        assert!(projection.knowledge_surface(&missing).is_none());
        assert!(projection.ui(&missing).is_none());
        assert!(projection.context(&missing).is_none());
    }

    #[test]
    fn projection_rejects_duplicate_and_unexpected_values() {
        let set = CapabilitySet::empty().unwrap();
        let source = CapabilitySource::builtin("a3s-code", digest('b')).unwrap();
        let id = CapabilityId::new(&source, CapabilityKind::Skill, "orphan").unwrap();
        let value = sample_skill_value("orphan");

        let duplicate = CapabilityProjection::new(
            Arc::clone(&set),
            [(id.clone(), value.clone()), (id.clone(), value.clone())],
        );
        assert!(matches!(
            duplicate,
            Err(CapabilityProjectionError::DuplicateValue { .. })
        ));

        let unexpected = CapabilityProjection::new(Arc::clone(&set), [(id, value)]);
        assert!(matches!(
            unexpected,
            Err(CapabilityProjectionError::UnexpectedValue { .. })
        ));
    }

    #[test]
    fn readiness_plan_mismatch_fails_closed() {
        let set = CapabilitySet::empty().unwrap();
        let other = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(2),
            Vec::<crate::capability::CapabilityContribution>::new(),
        )
        .unwrap();
        let readiness = Arc::new(CapabilityReadinessPlan::from_set(&other).unwrap());
        let err = CapabilityProjection::with_readiness(set, readiness, []);
        assert!(matches!(
            err,
            Err(CapabilityProjectionError::ReadinessPlanMismatch { .. })
        ));
    }

    #[test]
    fn projection_with_skill_covers_typed_lookups() {
        let source = CapabilitySource::builtin("a3s-code", digest('c')).unwrap();
        let descriptor = crate::capability::CapabilityDescriptor::new(
            &source,
            CapabilityKind::Skill,
            "coverage-skill",
            "coverage-skill",
            digest('d'),
            [],
        )
        .unwrap();
        let id = descriptor.id().clone();
        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(1),
            [crate::capability::CapabilityContribution::new(source, [descriptor]).unwrap()],
        )
        .unwrap();
        let value = sample_skill_value("coverage-skill");
        let projection =
            CapabilityProjection::new(Arc::clone(&set), [(id.clone(), value)]).unwrap();
        assert_eq!(projection.len(), 1);
        assert!(!projection.is_empty());
        assert!(projection.contains(&id));
        assert!(projection.skill(&id).is_some());
        assert!(projection.tool(&id).is_none());
        assert!(projection.agent(&id).is_none());
        assert!(projection.command(&id).is_none());
        assert!(projection.hook(&id).is_none());
        assert!(projection.mcp(&id).is_none());
        assert!(projection.knowledge(&id).is_none());
        assert!(projection.knowledge_surface(&id).is_none());
        assert!(projection.ui(&id).is_none());
        assert!(projection.context(&id).is_none());

        let catalog = CapabilityCatalog::new(Arc::clone(&projection));
        let rendered = format!("{catalog:?}");
        assert!(rendered.contains("CapabilityCatalog"));
        assert!(rendered.contains("pending_cleanup_batches"));
        let lease = catalog.pin();
        let lease_dbg = format!("{lease:?}");
        assert!(lease_dbg.contains("CapabilityProjectionLease"));
        assert_eq!(lease.projection().len(), 1);
        assert_eq!(lease.stamp().generation().get(), set.generation().get());
    }

    struct SlowEffect;
    struct FailingEffect;

    #[async_trait::async_trait]
    impl CapabilityEffect for SlowEffect {
        fn name(&self) -> &str {
            "slow-coverage"
        }

        async fn close(self: Box<Self>) -> Result<(), crate::capability::CapabilityEffectError> {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl CapabilityEffect for FailingEffect {
        fn name(&self) -> &str {
            "failing-coverage"
        }

        async fn close(self: Box<Self>) -> Result<(), crate::capability::CapabilityEffectError> {
            Err(crate::capability::CapabilityEffectError::new("boom"))
        }
    }

    #[tokio::test]
    async fn drain_cleanup_reports_failed_and_timed_out_effects() {
        let set = CapabilitySet::empty().unwrap();
        let projection = CapabilityProjection::new(Arc::clone(&set), []).unwrap();
        let catalog = CapabilityCatalog::new(projection);

        let failing: Box<dyn crate::capability::CapabilityEffect> = Box::new(FailingEffect);
        assert_eq!(failing.name(), "failing-coverage");
        catalog.inner.enqueue_rollback(vec![failing]);
        let failed = catalog.drain_cleanup().await;
        assert_eq!(failed.effects_failed, 1);
        assert!(!failed.is_clean());

        let slow: Box<dyn crate::capability::CapabilityEffect> = Box::new(SlowEffect);
        assert_eq!(slow.name(), "slow-coverage");
        catalog
            .inner
            .enqueue_rollback(vec![slow, Box::new(SlowEffect), Box::new(SlowEffect)]);
        let policy = ScopeClosePolicy::new(std::time::Duration::from_millis(5)).expect("policy");
        let timed_out = catalog.drain_cleanup_with_policy(policy).await;
        assert!(
            timed_out.effects_timed_out > 0,
            "expected timeout accounting, got {timed_out:?}"
        );
    }

    struct CoverageCommand;

    impl crate::commands::SlashCommand for CoverageCommand {
        fn name(&self) -> &str {
            "coverage-cmd"
        }

        fn description(&self) -> &str {
            "projection accessor coverage"
        }

        fn execute(
            &self,
            _args: &str,
            _ctx: &crate::commands::CommandContext,
        ) -> crate::commands::CommandOutput {
            crate::commands::CommandOutput::text("ok")
        }
    }

    struct CoverageHookHandler;

    impl crate::hooks::HookHandler for CoverageHookHandler {
        fn handle(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResponse {
            crate::hooks::HookResponse::continue_()
        }
    }

    struct CoverageKnowledgeProvider;

    #[async_trait::async_trait]
    impl crate::cognitive_context::CognitiveContextProvider for CoverageKnowledgeProvider {
        fn name(&self) -> &str {
            "coverage-knowledge"
        }

        async fn query(
            &self,
            _request: &crate::cognitive_context::CognitiveContextRequestV1,
        ) -> crate::cognitive_context::CognitiveContextResult<
            crate::cognitive_context::CognitiveContextResponseV1,
        > {
            Err(crate::cognitive_context::CognitiveContextError::Provider(
                "coverage provider is query-less".into(),
            ))
        }
    }

    fn sample_knowledge_value() -> CapabilityValue {
        let generation_digest =
            "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
        let knowledge = crate::cognitive_context::CognitiveKnowledgeBindingV1::new(
            "domain-knowledge",
            "0.2",
            "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
            7,
            generation_digest,
        )
        .unwrap();
        let binding = crate::cognitive_context::CognitivePackageBindingV1::new(
            "contra-sense/handbook",
            "0.1.0",
            7,
            generation_digest,
            "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
            knowledge,
            crate::cognitive_context::CognitiveContextLimits::default(),
        )
        .unwrap();
        CapabilityValue::Knowledge(Arc::new(
            crate::cognitive_context::CognitiveContextSession::new(
                binding,
                Arc::new(CoverageKnowledgeProvider),
            )
            .unwrap(),
        ))
    }

    #[tokio::test]
    async fn projection_typed_some_accessors_hit_each_capability_arm() {
        let source = CapabilitySource::builtin("a3s-code", digest('e')).unwrap();
        let kinds = [
            (CapabilityKind::Agent, "coverage-agent", "coverage-agent"),
            (CapabilityKind::Command, "coverage-cmd", "coverage-cmd"),
            (CapabilityKind::Hook, "coverage-hook", "coverage-hook"),
            (CapabilityKind::Mcp, "coverage-mcp", "coverage-mcp"),
            (
                CapabilityKind::Knowledge,
                "coverage-knowledge",
                "coverage-knowledge",
            ),
            (
                CapabilityKind::Context,
                "coverage-context",
                "coverage-context",
            ),
        ];
        let descriptors = kinds
            .iter()
            .enumerate()
            .map(|(index, (kind, name, public_name))| {
                crate::capability::CapabilityDescriptor::new(
                    &source,
                    *kind,
                    *name,
                    *public_name,
                    digest(char::from_digit((index + 1) as u32, 16).unwrap_or('f')),
                    [],
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let ids = descriptors
            .iter()
            .map(|descriptor| descriptor.id().clone())
            .collect::<Vec<_>>();
        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(3),
            [crate::capability::CapabilityContribution::new(source, descriptors).unwrap()],
        )
        .unwrap();

        let (mcp_binding, _transport, _client) = crate::mcp::test_support::ready_binding(
            "coverage-mcp",
            "v1",
            vec![crate::mcp::test_support::mcp_tool("ping", "ping")],
        )
        .await;
        let values = [
            (
                ids[0].clone(),
                CapabilityValue::Agent(Arc::new(crate::subagent::AgentDefinition::new(
                    "coverage-agent",
                    "projection coverage",
                ))),
            ),
            (
                ids[1].clone(),
                CapabilityValue::Command(Arc::new(CoverageCommand)),
            ),
            (
                ids[2].clone(),
                CapabilityValue::Hook(Arc::new(crate::hooks::HookBinding::new(
                    crate::hooks::Hook::new(
                        "coverage-hook",
                        crate::hooks::HookEventType::PreToolUse,
                    ),
                    Arc::new(CoverageHookHandler),
                ))),
            ),
            (ids[3].clone(), CapabilityValue::Mcp(mcp_binding)),
            (ids[4].clone(), sample_knowledge_value()),
            (
                ids[5].clone(),
                CapabilityValue::Context(Arc::new(crate::context::StaticContextProvider::new(
                    "coverage-context",
                ))),
            ),
        ];
        let projection = CapabilityProjection::new(Arc::clone(&set), values).unwrap();
        assert!(projection.agent(&ids[0]).is_some());
        assert_eq!(projection.agent(&ids[0]).unwrap().name, "coverage-agent");
        assert!(projection.command(&ids[1]).is_some());
        let command = projection.command(&ids[1]).unwrap();
        assert_eq!(command.name(), "coverage-cmd");
        assert_eq!(command.description(), "projection accessor coverage");
        let command_ctx = crate::commands::CommandContext {
            session_id: "coverage".into(),
            workspace: "/tmp".into(),
            model: "test".into(),
            history_len: 0,
            total_tokens: 0,
            total_cost: 0.0,
            tool_names: vec![],
            mcp_servers: vec![],
        };
        assert_eq!(command.execute("", &command_ctx).text, "ok");
        assert!(projection.hook(&ids[2]).is_some());
        assert_eq!(projection.hook(&ids[2]).unwrap().hook().id, "coverage-hook");
        let hook_response = CoverageHookHandler.handle(&crate::hooks::HookEvent::SessionStart(
            crate::hooks::SessionStartEvent {
                session_id: "coverage".into(),
                system_prompt: None,
                model_provider: "test".into(),
                model_name: "coverage".into(),
            },
        ));
        assert_eq!(hook_response.action, crate::hooks::HookAction::Continue);
        assert!(projection.mcp(&ids[3]).is_some());
        assert_eq!(
            projection.mcp(&ids[3]).unwrap().server_name(),
            "coverage-mcp"
        );
        assert!(projection.knowledge(&ids[4]).is_some());
        assert_eq!(
            projection.knowledge(&ids[4]).unwrap().provider_name(),
            "coverage-knowledge"
        );
        let knowledge_provider = CoverageKnowledgeProvider;
        assert_eq!(
            crate::cognitive_context::CognitiveContextProvider::name(&knowledge_provider),
            "coverage-knowledge"
        );
        let knowledge_binding = projection.knowledge(&ids[4]).unwrap().binding().clone();
        let knowledge_request = crate::cognitive_context::CognitiveContextRequestV1::new(
            "coverage-session",
            "coverage query",
            knowledge_binding,
        )
        .expect("knowledge request");
        let knowledge_err = knowledge_provider
            .query(&knowledge_request)
            .await
            .expect_err("coverage provider stays query-less");
        assert!(matches!(
            knowledge_err,
            crate::cognitive_context::CognitiveContextError::Provider(_)
        ));
        assert!(projection.context(&ids[5]).is_some());
        assert_eq!(
            projection.context(&ids[5]).unwrap().name(),
            "coverage-context"
        );
        // Wrong-kind lookups stay None.
        assert!(projection.skill(&ids[0]).is_none());
        assert!(projection.ui(&ids[1]).is_none());
        assert!(projection.tool(&ids[5]).is_none());
        assert!(projection.knowledge_surface(&ids[4]).is_none());
    }

    #[test]
    fn projection_rejects_ui_surface_digest_and_dependency_mismatches() {
        let source = CapabilitySource::builtin("a3s-code", digest('9')).unwrap();
        let document = crate::capability::UiDocument::new(
            crate::capability::UiAsset::new(
                crate::capability::UiAssetKind::Html,
                "<!doctype html><main>ui</main>",
            )
            .unwrap(),
            [],
            [],
        )
        .unwrap();
        let ui = crate::capability::UiBinding::new(crate::capability::UiBindingSpec {
            public_name: "ui-surface".to_owned(),
            title: "UI".to_owned(),
            description: "surface coverage".to_owned(),
            icon: "panel-top".to_owned(),
            order: 1,
            document,
        })
        .unwrap();
        let bad_digest = digest('a');
        let descriptor = crate::capability::CapabilityDescriptor::new(
            &source,
            CapabilityKind::Ui,
            "ui-surface",
            "ui-surface",
            bad_digest,
            [],
        )
        .unwrap();
        let id = descriptor.id().clone();
        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(5),
            [
                crate::capability::CapabilityContribution::new(source.clone(), [descriptor])
                    .unwrap(),
            ],
        )
        .unwrap();
        let err = CapabilityProjection::new(
            Arc::clone(&set),
            [(id, CapabilityValue::Ui(Arc::new(ui.clone())))],
        );
        assert!(matches!(
            err,
            Err(CapabilityProjectionError::SurfaceDigestMismatch { .. })
        ));

        let agent_dep = CapabilityId::new(&source, CapabilityKind::Agent, "blocked-dep").unwrap();
        let agent_descriptor = crate::capability::CapabilityDescriptor::new(
            &source,
            CapabilityKind::Agent,
            "blocked-dep",
            "blocked-dep",
            digest('b'),
            [],
        )
        .unwrap();
        let agent_id = agent_descriptor.id().clone();
        let descriptor = crate::capability::CapabilityDescriptor::new(
            &source,
            CapabilityKind::Ui,
            "ui-surface",
            "ui-surface",
            ui.surface_digest().clone(),
            [agent_dep],
        )
        .unwrap();
        let id = descriptor.id().clone();
        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(6),
            [crate::capability::CapabilityContribution::new(
                source,
                [agent_descriptor, descriptor],
            )
            .unwrap()],
        )
        .unwrap();
        let err = CapabilityProjection::new(
            Arc::clone(&set),
            [
                (
                    agent_id,
                    CapabilityValue::Agent(Arc::new(crate::subagent::AgentDefinition::new(
                        "blocked-dep",
                        "dependency present only to admit the ui descriptor",
                    ))),
                ),
                (id, CapabilityValue::Ui(Arc::new(ui))),
            ],
        );
        assert!(matches!(
            err,
            Err(CapabilityProjectionError::UnsupportedUiDependencyKind { .. })
        ));
    }

    #[test]
    fn projection_rejects_public_name_mismatch() {
        let source = CapabilitySource::builtin("a3s-code", digest('7')).unwrap();
        let descriptor = crate::capability::CapabilityDescriptor::new(
            &source,
            CapabilityKind::Skill,
            "expected-name",
            "expected-name",
            digest('8'),
            [],
        )
        .unwrap();
        let id = descriptor.id().clone();
        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(4),
            [crate::capability::CapabilityContribution::new(source, [descriptor]).unwrap()],
        )
        .unwrap();
        let err =
            CapabilityProjection::new(Arc::clone(&set), [(id, sample_skill_value("actual-name"))]);
        assert!(matches!(
            err,
            Err(CapabilityProjectionError::PublicNameMismatch { .. })
        ));
    }
}
