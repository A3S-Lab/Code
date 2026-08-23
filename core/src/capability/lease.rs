use std::marker::PhantomData;

use tokio_util::sync::CancellationToken;

use super::scope::ScopeInner;
use super::{
    CapabilityCeiling, CapabilityDescriptor, CapabilityId, CapabilityScopeError, CapabilityScopeId,
    CapabilityScopeKind, CodeCatalogGeneration, ScopeKind, Sha256Digest, UseCapabilityGeneration,
};

/// Host adapter for the exact non-clone lease returned by A3S Use.
///
/// Implementations retain the real
/// `a3s_use::capability_registry::CapabilitySnapshotLease` (or an equivalent
/// host wrapper) as part of `self`. Code consumes the value at Run admission
/// and drops it only after every child, task, and effect has settled.
pub trait RetainedUseGeneration: Send + Sync + 'static {
    fn use_generation(&self) -> &UseCapabilityGeneration;
}

impl<T> RetainedUseGeneration for Box<T>
where
    T: RetainedUseGeneration + ?Sized,
{
    fn use_generation(&self) -> &UseCapabilityGeneration {
        self.as_ref().use_generation()
    }
}

/// Borrowed capability access tied to one typed scope owner.
///
/// A lease cannot be returned after its owner is dropped:
///
/// ```compile_fail
/// use a3s_code_core::capability::{CapabilityLease, CapabilityScope, Run};
///
/// fn escape<'a>(scope: CapabilityScope<Run>) -> CapabilityLease<'a, Run> {
///     scope.lease().unwrap()
/// }
/// ```
///
/// Marker types also prevent a narrower Turn lease from entering a Run-only
/// API:
///
/// ```compile_fail
/// use a3s_code_core::capability::{CapabilityLease, Run, Turn};
///
/// fn needs_run(_lease: CapabilityLease<'_, Run>) {}
/// fn wrong_scope(lease: CapabilityLease<'_, Turn>) {
///     needs_run(lease);
/// }
/// ```
#[must_use = "a capability lease borrows and pins one active scope"]
pub struct CapabilityLease<'scope, K: ScopeKind> {
    inner: &'scope ScopeInner,
    _kind: PhantomData<K>,
}

impl<'scope, K: ScopeKind> CapabilityLease<'scope, K> {
    pub(super) fn new(inner: &'scope ScopeInner) -> Self {
        Self {
            inner,
            _kind: PhantomData,
        }
    }

    pub fn scope_id(&self) -> &CapabilityScopeId {
        self.inner.id()
    }

    pub const fn kind(&self) -> CapabilityScopeKind {
        K::KIND
    }

    pub fn parent_id(&self) -> Option<&CapabilityScopeId> {
        self.inner.parent_id()
    }

    pub fn catalog_generation(&self) -> CodeCatalogGeneration {
        self.inner.set().generation()
    }

    pub fn catalog_digest(&self) -> &Sha256Digest {
        self.inner.set().digest()
    }

    pub fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.inner.use_generation()
    }

    pub fn ceiling(&self) -> &CapabilityCeiling {
        self.inner.ceiling()
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.inner.supervisor_cancellation()
    }

    pub fn get(
        &self,
        id: &CapabilityId,
    ) -> Result<Option<&'scope CapabilityDescriptor>, CapabilityScopeError> {
        self.inner.ensure_active()?;
        if !self.inner.ceiling().allows(id) {
            return Ok(None);
        }
        Ok(self.inner.set().get(id))
    }

    pub fn contains(&self, id: &CapabilityId) -> Result<bool, CapabilityScopeError> {
        self.get(id).map(|descriptor| descriptor.is_some())
    }

    pub fn iter(
        &self,
    ) -> Result<
        impl Iterator<Item = (&'scope CapabilityId, &'scope CapabilityDescriptor)> + 'scope,
        CapabilityScopeError,
    > {
        self.inner.ensure_active()?;
        let ceiling = self.inner.ceiling();
        Ok(self
            .inner
            .set()
            .iter()
            .filter(move |(id, _)| ceiling.allows(id)))
    }
}

impl<K: ScopeKind> std::fmt::Debug for CapabilityLease<'_, K> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityLease")
            .field("scope_id", &self.scope_id())
            .field("kind", &K::KIND)
            .field("catalog_digest", &self.catalog_digest())
            .finish()
    }
}
