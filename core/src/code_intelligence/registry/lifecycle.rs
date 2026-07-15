use std::{any::Any, collections::HashSet};

use futures::FutureExt;

use super::*;

type RetiredEntry<R, E> = (RegistryKey, Arc<RegistryEntry<R, E>>, Arc<R>);

pub(super) fn spawn_factory<R, E, F, Fut>(
    registry: Arc<RegistryInner<R, E>>,
    key: RegistryKey,
    entry: Arc<RegistryEntry<R, E>>,
    factory: F,
) where
    R: Send + Sync + 'static,
    E: Send + Sync + 'static,
    F: FnOnce(RegistryKey) -> Fut + Send + 'static,
    Fut: Future<Output = Result<R, E>> + Send + 'static,
{
    tokio::spawn(async move {
        let factory_key = key.clone();
        let outcome = std::panic::AssertUnwindSafe(async move { factory(factory_key).await })
            .catch_unwind()
            .await;
        match outcome {
            Ok(Ok(runtime)) => {
                let mut state = mutex_lock(&entry.state);
                if matches!(*state, EntryState::Starting) {
                    *state = EntryState::Ready {
                        runtime: Arc::new(runtime),
                        leases: 0,
                        idle_since: Some(Instant::now()),
                        last_access: next_access(&registry),
                    };
                }
            }
            Ok(Err(error)) => complete_factory_failure(
                &registry,
                &key,
                &entry,
                StartFailure::Factory(Arc::new(error)),
            ),
            Err(payload) => complete_factory_failure(
                &registry,
                &key,
                &entry,
                StartFailure::Panicked(panic_message(payload)),
            ),
        }
        entry.changed.notify_waiters();
    });
}

fn complete_factory_failure<R, E>(
    registry: &Arc<RegistryInner<R, E>>,
    key: &RegistryKey,
    entry: &Arc<RegistryEntry<R, E>>,
    failure: StartFailure<E>,
) {
    {
        let mut state = mutex_lock(&entry.state);
        if matches!(*state, EntryState::Starting) {
            *state = EntryState::Failed(failure);
        }
    }
    let mut registry = mutex_lock(&registry.state);
    if registry
        .entries
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, entry))
    {
        registry.entries.remove(key);
    }
}

pub(super) fn retire_idle_entries<R, E>(
    registry: &Arc<RegistryInner<R, E>>,
    now: Instant,
) -> Vec<RetiredEntry<R, E>> {
    let mut state = mutex_lock(&registry.state);
    let mut idle = state
        .entries
        .iter()
        .filter_map(|(key, entry)| {
            if Arc::strong_count(entry) != 1 {
                return None;
            }
            let entry_state = mutex_lock(&entry.state);
            let EntryState::Ready {
                leases: 0,
                idle_since: Some(idle_since),
                last_access,
                ..
            } = &*entry_state
            else {
                return None;
            };
            let expired = now.checked_duration_since(*idle_since).unwrap_or_default()
                >= registry.config.idle_ttl;
            Some((key.clone(), *last_access, expired))
        })
        .collect::<Vec<_>>();
    idle.sort_by_key(|(_, last_access, _)| *last_access);

    let mut selected = idle
        .iter()
        .filter(|(_, _, expired)| *expired)
        .map(|(key, _, _)| key.clone())
        .collect::<HashSet<_>>();
    let retained_idle = idle.len().saturating_sub(selected.len());
    let excess = retained_idle.saturating_sub(registry.config.max_idle_entries);
    for (key, _, _) in idle.iter().filter(|(_, _, expired)| !*expired).take(excess) {
        selected.insert(key.clone());
    }

    let mut retired = Vec::with_capacity(selected.len());
    for key in selected {
        let Some(entry) = state.entries.remove(&key) else {
            continue;
        };
        let runtime = {
            let mut entry_state = mutex_lock(&entry.state);
            match &*entry_state {
                EntryState::Ready {
                    runtime, leases: 0, ..
                } => {
                    let runtime = Arc::clone(runtime);
                    *entry_state = EntryState::ShuttingDown;
                    Some(runtime)
                }
                _ => None,
            }
        };
        if let Some(runtime) = runtime {
            retired.push((key, entry, runtime));
        } else {
            state.entries.insert(key, entry);
        }
    }
    retired
}

pub(super) async fn shutdown_entry<R, E>(
    registry: &Arc<RegistryInner<R, E>>,
    entry: &Arc<RegistryEntry<R, E>>,
) -> Option<RegistryShutdownFailure<E>>
where
    R: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    loop {
        let changed = entry.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let runtime = {
            let mut state = mutex_lock(&entry.state);
            match &*state {
                EntryState::Starting => None,
                EntryState::Ready { runtime, .. } => {
                    let runtime = Arc::clone(runtime);
                    *state = EntryState::ShuttingDown;
                    Some(runtime)
                }
                EntryState::Failed(_) => {
                    *state = EntryState::Stopped;
                    entry.changed.notify_waiters();
                    return None;
                }
                EntryState::ShuttingDown => None,
                EntryState::Stopped => return None,
            }
        };
        match runtime {
            Some(runtime) => return stop_runtime(registry, entry, runtime).await,
            None => changed.await,
        }
    }
}

pub(super) async fn stop_runtime<R, E>(
    registry: &Arc<RegistryInner<R, E>>,
    entry: &Arc<RegistryEntry<R, E>>,
    runtime: Arc<R>,
) -> Option<RegistryShutdownFailure<E>>
where
    R: Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    let shutdown = Arc::clone(&registry.shutdown);
    let outcome = std::panic::AssertUnwindSafe(async move { shutdown(runtime).await })
        .catch_unwind()
        .await;
    {
        let mut state = mutex_lock(&entry.state);
        *state = EntryState::Stopped;
    }
    entry.changed.notify_waiters();

    match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(RegistryShutdownFailure::Runtime(Arc::new(error))),
        Err(payload) => Some(RegistryShutdownFailure::Panicked {
            message: panic_message(payload),
        }),
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> Arc<str> {
    if let Some(message) = payload.downcast_ref::<&str>() {
        Arc::from(*message)
    } else if let Some(message) = payload.downcast_ref::<String>() {
        Arc::from(message.as_str())
    } else {
        Arc::from("unknown panic payload")
    }
}
