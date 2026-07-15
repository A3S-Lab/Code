use std::{
    collections::HashMap,
    fmt,
    future::Future,
    ops::Deref,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use tokio::sync::{Mutex as AsyncMutex, Notify};

mod lifecycle;
#[cfg(test)]
mod tests;

use lifecycle::{retire_idle_entries, shutdown_entry, spawn_factory, stop_runtime};

type ShutdownFuture<E> = Pin<Box<dyn Future<Output = Result<(), E>> + Send + 'static>>;
type ShutdownCallback<R, E> = dyn Fn(Arc<R>) -> ShutdownFuture<E> + Send + Sync + 'static;

/// Fully isolated identity of one workspace runtime.
///
/// Construction resolves symlinks and lexical aliases so equivalent roots
/// cannot create duplicate language processes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RegistryKey {
    isolation_scope: String,
    canonical_root: PathBuf,
    layout_hash: u64,
}

impl RegistryKey {
    pub(crate) async fn new(
        isolation_scope: impl Into<String>,
        workspace_root: impl AsRef<Path>,
        layout_hash: u64,
    ) -> Result<Self, RegistryKeyError> {
        let isolation_scope = isolation_scope.into();
        if isolation_scope.trim().is_empty() {
            return Err(RegistryKeyError::EmptyIsolationScope);
        }

        let supplied_root = workspace_root.as_ref().to_path_buf();
        let canonical_root = tokio::fs::canonicalize(&supplied_root)
            .await
            .map_err(|source| RegistryKeyError::Canonicalize {
                root: supplied_root,
                source,
            })?;
        let metadata = tokio::fs::metadata(&canonical_root)
            .await
            .map_err(|source| RegistryKeyError::Canonicalize {
                root: canonical_root.clone(),
                source,
            })?;
        if !metadata.is_dir() {
            return Err(RegistryKeyError::RootIsNotDirectory {
                root: canonical_root,
            });
        }

        Ok(Self {
            isolation_scope,
            canonical_root,
            layout_hash,
        })
    }

    #[cfg(test)]
    pub(crate) fn isolation_scope(&self) -> &str {
        &self.isolation_scope
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    #[cfg(test)]
    pub(crate) const fn layout_hash(&self) -> u64 {
        self.layout_hash
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegistryKeyError {
    #[error("Code Intelligence isolation scope cannot be empty")]
    EmptyIsolationScope,

    #[error("failed to canonicalize Code Intelligence workspace root {root:?}: {source}")]
    Canonicalize {
        root: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Code Intelligence workspace root is not a directory: {root:?}")]
    RootIsNotDirectory { root: PathBuf },
}

/// Bounds applied only to idle runtimes. Active leases are never evicted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegistryConfig {
    pub(crate) idle_ttl: Duration,
    pub(crate) max_idle_entries: usize,
}

impl RegistryConfig {
    pub(crate) const fn new(idle_ttl: Duration, max_idle_entries: usize) -> Self {
        Self {
            idle_ttl,
            max_idle_entries,
        }
    }
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self::new(Duration::from_secs(5 * 60), 8)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegistryAcquireError<E> {
    #[error("Code Intelligence registry is shut down")]
    ShuttingDown,

    #[error("Code Intelligence runtime initialization failed: {0}")]
    Factory(Arc<E>),

    #[error("Code Intelligence runtime initialization panicked: {message}")]
    FactoryPanicked { message: Arc<str> },

    #[error("Code Intelligence runtime lease limit was exhausted")]
    LeaseLimit,
}

impl<E> Clone for RegistryAcquireError<E> {
    fn clone(&self) -> Self {
        match self {
            Self::ShuttingDown => Self::ShuttingDown,
            Self::Factory(error) => Self::Factory(Arc::clone(error)),
            Self::FactoryPanicked { message } => Self::FactoryPanicked {
                message: Arc::clone(message),
            },
            Self::LeaseLimit => Self::LeaseLimit,
        }
    }
}

#[derive(Debug)]
pub(crate) enum RegistryShutdownFailure<E> {
    Runtime(Arc<E>),
    Panicked { message: Arc<str> },
}

#[derive(Debug)]
pub(crate) struct RegistryShutdownError<E> {
    pub(crate) key: RegistryKey,
    pub(crate) failure: RegistryShutdownFailure<E>,
}

#[derive(Debug)]
pub(crate) struct RegistryReport<E> {
    pub(crate) removed: Vec<RegistryKey>,
    pub(crate) errors: Vec<RegistryShutdownError<E>>,
}

enum StartFailure<E> {
    Factory(Arc<E>),
    Panicked(Arc<str>),
}

impl<E> Clone for StartFailure<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Factory(error) => Self::Factory(Arc::clone(error)),
            Self::Panicked(message) => Self::Panicked(Arc::clone(message)),
        }
    }
}

enum EntryObservation<E> {
    Starting,
    Ready,
    Failed(StartFailure<E>),
    Unavailable,
}

enum EntryState<R, E> {
    Starting,
    Ready {
        runtime: Arc<R>,
        leases: usize,
        idle_since: Option<Instant>,
        last_access: u64,
    },
    Failed(StartFailure<E>),
    ShuttingDown,
    Stopped,
}

struct RegistryEntry<R, E> {
    state: Mutex<EntryState<R, E>>,
    changed: Notify,
}

impl<R, E> RegistryEntry<R, E> {
    fn starting() -> Self {
        Self {
            state: Mutex::new(EntryState::Starting),
            changed: Notify::new(),
        }
    }
}

struct RegistryState<R, E> {
    accepting: bool,
    entries: HashMap<RegistryKey, Arc<RegistryEntry<R, E>>>,
}

struct RegistryInner<R, E> {
    config: RegistryConfig,
    state: Mutex<RegistryState<R, E>>,
    access_clock: AtomicU64,
    lifecycle: AsyncMutex<()>,
    shutdown: Arc<ShutdownCallback<R, E>>,
}

/// Workspace-scoped runtime registry with single-flight initialization.
pub(crate) struct LocalCodeIntelligenceRegistry<R, E> {
    inner: Arc<RegistryInner<R, E>>,
}

impl<R, E> Clone for LocalCodeIntelligenceRegistry<R, E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<R, E> LocalCodeIntelligenceRegistry<R, E>
where
    R: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    pub(crate) fn new<S, Fut>(config: RegistryConfig, shutdown: S) -> Self
    where
        S: Fn(Arc<R>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
    {
        let shutdown = Arc::new(move |runtime| Box::pin(shutdown(runtime)) as ShutdownFuture<E>);
        Self {
            inner: Arc::new(RegistryInner {
                config,
                state: Mutex::new(RegistryState {
                    accepting: true,
                    entries: HashMap::new(),
                }),
                access_clock: AtomicU64::new(0),
                lifecycle: AsyncMutex::new(()),
                shutdown,
            }),
        }
    }

    /// Acquire one runtime. Exactly one owned factory task is active per key.
    pub(crate) async fn acquire<F, Fut>(
        &self,
        key: RegistryKey,
        factory: F,
    ) -> Result<RuntimeLease<R, E>, RegistryAcquireError<E>>
    where
        F: FnOnce(RegistryKey) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, E>> + Send + 'static,
    {
        let (entry, starts_factory) = {
            let mut state = mutex_lock(&self.inner.state);
            if !state.accepting {
                return Err(RegistryAcquireError::ShuttingDown);
            }
            match state.entries.get(&key) {
                Some(entry) => (Arc::clone(entry), false),
                None => {
                    let entry = Arc::new(RegistryEntry::starting());
                    state.entries.insert(key.clone(), Arc::clone(&entry));
                    (entry, true)
                }
            }
        };

        if starts_factory {
            spawn_factory(
                Arc::clone(&self.inner),
                key.clone(),
                Arc::clone(&entry),
                factory,
            );
        }
        self.wait_for_lease(key, entry).await
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        mutex_lock(&self.inner.state).entries.len()
    }

    #[cfg(test)]
    pub(crate) fn is_accepting(&self) -> bool {
        mutex_lock(&self.inner.state).accepting
    }

    pub(crate) async fn cleanup_idle(&self) -> RegistryReport<E> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let retired = retire_idle_entries(&self.inner, Instant::now());
        let mut report = RegistryReport {
            removed: retired.iter().map(|(key, _, _)| key.clone()).collect(),
            errors: Vec::new(),
        };
        for (key, entry, runtime) in retired {
            if let Some(failure) = stop_runtime(&self.inner, &entry, runtime).await {
                report.errors.push(RegistryShutdownError { key, failure });
            }
        }
        report
    }

    /// Permanently stop admission and shut down every registered runtime.
    pub(crate) async fn shutdown_all(&self) -> RegistryReport<E> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let entries = {
            let mut state = mutex_lock(&self.inner.state);
            state.accepting = false;
            std::mem::take(&mut state.entries)
                .into_iter()
                .collect::<Vec<_>>()
        };
        let mut report = RegistryReport {
            removed: entries.iter().map(|(key, _)| key.clone()).collect(),
            errors: Vec::new(),
        };
        for (key, entry) in entries {
            if let Some(failure) = shutdown_entry(&self.inner, &entry).await {
                report.errors.push(RegistryShutdownError { key, failure });
            }
        }
        report
    }

    async fn wait_for_lease(
        &self,
        key: RegistryKey,
        entry: Arc<RegistryEntry<R, E>>,
    ) -> Result<RuntimeLease<R, E>, RegistryAcquireError<E>> {
        loop {
            let changed = entry.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let observation = {
                let state = mutex_lock(&entry.state);
                match &*state {
                    EntryState::Starting => EntryObservation::Starting,
                    EntryState::Ready { .. } => EntryObservation::Ready,
                    EntryState::Failed(failure) => EntryObservation::Failed(failure.clone()),
                    EntryState::ShuttingDown | EntryState::Stopped => EntryObservation::Unavailable,
                }
            };
            match observation {
                EntryObservation::Starting => changed.await,
                EntryObservation::Ready => return self.try_lease(&key, &entry),
                EntryObservation::Failed(StartFailure::Factory(error)) => {
                    return Err(RegistryAcquireError::Factory(error));
                }
                EntryObservation::Failed(StartFailure::Panicked(message)) => {
                    return Err(RegistryAcquireError::FactoryPanicked { message });
                }
                EntryObservation::Unavailable => {
                    return Err(RegistryAcquireError::ShuttingDown);
                }
            }
        }
    }

    fn try_lease(
        &self,
        key: &RegistryKey,
        entry: &Arc<RegistryEntry<R, E>>,
    ) -> Result<RuntimeLease<R, E>, RegistryAcquireError<E>> {
        let registry = mutex_lock(&self.inner.state);
        if !registry.accepting
            || !registry
                .entries
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return Err(RegistryAcquireError::ShuttingDown);
        }

        let mut state = mutex_lock(&entry.state);
        let EntryState::Ready {
            runtime,
            leases,
            idle_since,
            last_access,
        } = &mut *state
        else {
            return Err(RegistryAcquireError::ShuttingDown);
        };
        *leases = leases
            .checked_add(1)
            .ok_or(RegistryAcquireError::LeaseLimit)?;
        *idle_since = None;
        *last_access = next_access(&self.inner);
        Ok(RuntimeLease {
            key: key.clone(),
            runtime: Arc::clone(runtime),
            entry: Arc::clone(entry),
            registry: Arc::clone(&self.inner),
        })
    }
}

/// RAII ownership of one runtime consumer.
pub(crate) struct RuntimeLease<R, E> {
    key: RegistryKey,
    runtime: Arc<R>,
    entry: Arc<RegistryEntry<R, E>>,
    registry: Arc<RegistryInner<R, E>>,
}

impl<R, E> fmt::Debug for RuntimeLease<R, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeLease")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<R, E> RuntimeLease<R, E> {
    #[cfg(test)]
    pub(crate) fn key(&self) -> &RegistryKey {
        &self.key
    }
}

impl<R, E> Deref for RuntimeLease<R, E> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        self.runtime.as_ref()
    }
}

impl<R, E> Drop for RuntimeLease<R, E> {
    fn drop(&mut self) {
        let mut state = mutex_lock(&self.entry.state);
        let EntryState::Ready {
            leases,
            idle_since,
            last_access,
            ..
        } = &mut *state
        else {
            return;
        };
        if *leases == 0 {
            return;
        }
        *leases -= 1;
        *last_access = next_access(&self.registry);
        if *leases == 0 {
            *idle_since = Some(Instant::now());
        }
    }
}

fn next_access<R, E>(registry: &RegistryInner<R, E>) -> u64 {
    registry
        .access_clock
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
