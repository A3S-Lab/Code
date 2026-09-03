//! Hook Engine
//!
//! Core engine responsible for managing and executing hooks.

use super::{
    Hook, HookAction, HookBinding, HookEvent, HookExecutor, HookHandler, HookOutcome, HookResponse,
    HookResult,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};
use tokio::sync::mpsc;

use crate::error::{read_or_recover, write_or_recover};

pub(crate) type HookTaskFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub(crate) trait HookTaskDispatcher: Send + Sync {
    fn dispatch(&self, name: &'static str, task: HookTaskFuture) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
#[error("projected Hook name '{name}' conflicts with the compatibility registry")]
pub(crate) struct HookEngineSnapshotError {
    name: String,
}

impl HookEngineSnapshotError {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// Hook engine
pub struct HookEngine {
    /// Registered hooks
    hooks: Arc<RwLock<HashMap<String, Arc<Hook>>>>,

    /// Hook handlers (registered by SDK)
    handlers: Arc<RwLock<HashMap<String, Arc<dyn HookHandler>>>>,

    /// Event sender channel (for SDK listeners)
    event_tx: Option<mpsc::Sender<HookEvent>>,

    /// Run-owned dispatcher for detached observational handlers.
    task_dispatcher: OnceLock<Arc<dyn HookTaskDispatcher>>,
}

impl std::fmt::Debug for HookEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookEngine")
            .field("hooks_count", &read_or_recover(&self.hooks).len())
            .field("handlers_count", &read_or_recover(&self.handlers).len())
            .field("has_event_channel", &self.event_tx.is_some())
            .field("has_task_dispatcher", &self.task_dispatcher.get().is_some())
            .finish()
    }
}

impl Default for HookEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HookEngine {
    /// Create a new hook engine
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(RwLock::new(HashMap::new())),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            task_dispatcher: OnceLock::new(),
        }
    }

    /// Set the event sender channel
    pub fn with_event_channel(mut self, tx: mpsc::Sender<HookEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Register a hook
    pub fn register(&self, hook: Hook) {
        let mut hooks = write_or_recover(&self.hooks);
        hooks.insert(hook.id.clone(), Arc::new(hook));
    }

    /// Unregister a hook
    pub fn unregister(&self, hook_id: &str) -> Option<Hook> {
        let mut hooks = write_or_recover(&self.hooks);
        hooks.remove(hook_id).map(|hook| (*hook).clone())
    }

    /// Register a handler
    pub fn register_handler(&self, hook_id: &str, handler: Arc<dyn HookHandler>) {
        drop(self.replace_handler(hook_id, handler));
    }

    /// Unregister a handler
    pub fn unregister_handler(&self, hook_id: &str) {
        drop(self.take_handler(hook_id));
    }

    pub(crate) fn replace_handler(
        &self,
        hook_id: &str,
        handler: Arc<dyn HookHandler>,
    ) -> Option<Arc<dyn HookHandler>> {
        write_or_recover(&self.handlers).insert(hook_id.to_string(), handler)
    }

    pub(crate) fn take_handler(&self, hook_id: &str) -> Option<Arc<dyn HookHandler>> {
        write_or_recover(&self.handlers).remove(hook_id)
    }

    /// Atomically replace one complete compatibility Hook registration.
    pub(crate) fn register_registration(
        &self,
        hook: Hook,
        handler: Option<Arc<dyn HookHandler>>,
    ) -> (Option<Arc<Hook>>, Option<Arc<dyn HookHandler>>) {
        let hook_id = hook.id.clone();
        let mut hooks = write_or_recover(&self.hooks);
        let mut handlers = write_or_recover(&self.handlers);
        let retired_hook = hooks.insert(hook_id.clone(), Arc::new(hook));
        let retired_handler = match handler {
            Some(handler) => handlers.insert(hook_id, handler),
            None => handlers.remove(&hook_id),
        };
        (retired_hook, retired_handler)
    }

    /// Atomically remove one complete compatibility Hook registration.
    pub(crate) fn unregister_registration(
        &self,
        hook_id: &str,
    ) -> (Option<Arc<Hook>>, Option<Arc<dyn HookHandler>>) {
        let mut hooks = write_or_recover(&self.hooks);
        let mut handlers = write_or_recover(&self.handlers);
        let retired_handler = handlers.remove(hook_id);
        let retired_hook = hooks.remove(hook_id);
        (retired_hook, retired_handler)
    }

    /// Freeze the compatibility registry and merge one projected generation.
    ///
    /// Compatibility names always participate in conflict detection. They are
    /// copied into the executable snapshot only when the in-process engine is
    /// the Session's active compatibility executor.
    pub(crate) fn snapshot_with_external_hooks(
        &self,
        external: impl IntoIterator<Item = Arc<HookBinding>>,
        include_compatibility: bool,
    ) -> Result<Self, HookEngineSnapshotError> {
        // Preserve one lock order everywhere both maps are observed.
        let compatibility_hooks = read_or_recover(&self.hooks);
        let compatibility_handlers = read_or_recover(&self.handlers);
        let compatibility_names = compatibility_hooks
            .keys()
            .chain(compatibility_handlers.keys())
            .cloned()
            .collect::<HashSet<_>>();
        let mut hooks = if include_compatibility {
            compatibility_hooks.clone()
        } else {
            HashMap::new()
        };
        let mut handlers = if include_compatibility {
            compatibility_handlers.clone()
        } else {
            HashMap::new()
        };
        let mut projected_names = HashSet::new();

        for binding in external {
            let name = binding.hook().id.clone();
            if compatibility_names.contains(&name) || !projected_names.insert(name.clone()) {
                return Err(HookEngineSnapshotError { name });
            }
            hooks.insert(name.clone(), Arc::clone(binding.hook_arc()));
            handlers.insert(name, Arc::clone(binding.handler_arc()));
        }

        Ok(Self {
            hooks: Arc::new(RwLock::new(hooks)),
            handlers: Arc::new(RwLock::new(handlers)),
            event_tx: include_compatibility
                .then(|| self.event_tx.clone())
                .flatten(),
            task_dispatcher: OnceLock::new(),
        })
    }

    pub(crate) fn attach_task_dispatcher(
        &self,
        dispatcher: Arc<dyn HookTaskDispatcher>,
    ) -> Result<(), Arc<dyn HookTaskDispatcher>> {
        self.task_dispatcher.set(dispatcher)
    }

    /// Get all hooks matching an event (sorted by priority)
    pub fn matching_hooks(&self, event: &HookEvent) -> Vec<Hook> {
        self.matching_hook_arcs(event)
            .into_iter()
            .map(|hook| (*hook).clone())
            .collect()
    }

    fn matching_hook_arcs(&self, event: &HookEvent) -> Vec<Arc<Hook>> {
        let hooks = read_or_recover(&self.hooks);
        let mut matching: Vec<Arc<Hook>> = hooks
            .values()
            .filter(|h| h.matches(event))
            .cloned()
            .collect();

        // Sort by priority (lower values = higher priority)
        matching.sort_by(|left, right| {
            left.config
                .priority
                .cmp(&right.config.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        matching
    }

    /// Fire an event and get the result
    pub async fn fire(&self, event: &HookEvent) -> HookResult {
        self.fire_outcome(event).await.into()
    }

    /// Fire an event while preserving retry explanations.
    pub async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        self.fire_outcome_with_policy(event, true).await
    }

    /// Fire an event from an already supervised observational task.
    ///
    /// Per-Hook asynchronous flags execute inline here so no nested detached
    /// task can race the owning Run's close transition.
    pub(crate) async fn fire_outcome_inline_observers(&self, event: &HookEvent) -> HookOutcome {
        self.fire_outcome_with_policy(event, false).await
    }

    async fn fire_outcome_with_policy(
        &self,
        event: &HookEvent,
        detach_observational_handlers: bool,
    ) -> HookOutcome {
        // Send event to channel if available
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event.clone()).await;
        }

        // Get matching hooks
        let matching_hooks = self.matching_hook_arcs(event);

        if matching_hooks.is_empty() {
            return HookOutcome::Continue(None);
        }

        // Execute each hook
        let mut last_modified: Option<serde_json::Value> = None;
        for hook in matching_hooks {
            let result = self
                .execute_hook(&hook, event, detach_observational_handlers)
                .await;

            match result {
                HookOutcome::Continue(modified) => {
                    // Track the last modification — continue to subsequent hooks
                    if modified.is_some() {
                        last_modified = modified;
                    }
                }
                block @ HookOutcome::Block { .. } => return block,
                retry @ HookOutcome::Retry { .. } => return retry,
                HookOutcome::Skip => return HookOutcome::Continue(None),
                escalate @ HookOutcome::Escalate { .. } => return escalate,
            }
        }

        HookOutcome::Continue(last_modified)
    }

    /// Execute a single hook
    async fn execute_hook(
        &self,
        hook: &Hook,
        event: &HookEvent,
        detach_observational_handlers: bool,
    ) -> HookOutcome {
        let is_gate = Self::is_gating_event(event);

        // Find handler
        let handler = {
            let handlers = read_or_recover(&self.handlers);
            handlers.get(&hook.id).cloned()
        };

        match handler {
            Some(h) => {
                // A gating hook must produce a decision before the protected
                // operation starts. Treat `async_execution` as best-effort only
                // for observational hooks; otherwise a configuration flag could
                // silently bypass a security policy.
                if hook.config.async_execution && !is_gate && detach_observational_handlers {
                    let hook_id = hook.id.clone();
                    let event = event.clone();
                    let event_type = event.event_type();
                    let task: HookTaskFuture = Box::pin(async move {
                        let response = tokio::task::spawn_blocking(move || {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                h.try_handle(&event)
                            }))
                        })
                        .await;
                        match response {
                            Ok(Ok(Ok(_))) => {}
                            Ok(Ok(Err(error))) => tracing::warn!(
                                hook_id = %hook_id,
                                event_type = %event_type,
                                failure = %error,
                                "Asynchronous observational hook handler failed"
                            ),
                            Ok(Err(_)) => tracing::warn!(
                                hook_id = %hook_id,
                                event_type = %event_type,
                                "Asynchronous observational hook handler panicked"
                            ),
                            Err(error) => tracing::warn!(
                                hook_id = %hook_id,
                                event_type = %event_type,
                                failure = %error,
                                "Asynchronous observational hook task failed"
                            ),
                        }
                    });
                    if let Some(dispatcher) = self.task_dispatcher.get() {
                        if let Err(error) = dispatcher.dispatch("hook.observer", task) {
                            tracing::warn!(
                                hook_id = %hook.id,
                                event_type = %event_type,
                                failure = %error,
                                "Asynchronous observational hook could not be supervised"
                            );
                        }
                    } else {
                        tokio::spawn(task);
                    }
                    return HookOutcome::Continue(None);
                }

                let timeout = std::time::Duration::from_millis(hook.config.timeout_ms);
                let event_for_handler = event.clone();
                let mut task =
                    tokio::task::spawn_blocking(move || h.try_handle(&event_for_handler));

                match tokio::time::timeout(timeout, &mut task).await {
                    Ok(Ok(Ok(response))) => self.response_to_outcome(response),
                    Ok(Ok(Err(error))) => self.handler_failure(hook, event, error),
                    Ok(Err(error)) => self.handler_failure(
                        hook,
                        event,
                        format!("handler terminated unexpectedly: {error}"),
                    ),
                    Err(_) => {
                        // `spawn_blocking` work cannot always be cancelled once
                        // running. A Run snapshot transfers the join handle to
                        // its capability supervisor so the exact Use lease is
                        // retained through bounded settlement.
                        task.abort();
                        if !detach_observational_handlers {
                            // The caller is already one supervised
                            // observational task. Settle here instead of trying
                            // to register nested work after Run close may have
                            // entered its Closing state.
                            if let Err(error) = task.await {
                                if !error.is_cancelled() {
                                    tracing::warn!(
                                        hook_id = %hook.id,
                                        event_type = %event.event_type(),
                                        failure = %error,
                                        "Timed-out observational Hook handler failed while settling"
                                    );
                                }
                            }
                        } else if let Some(dispatcher) = self.task_dispatcher.get() {
                            let hook_id = hook.id.clone();
                            let event_type = event.event_type();
                            let settle: HookTaskFuture = Box::pin(async move {
                                if let Err(error) = task.await {
                                    if !error.is_cancelled() {
                                        tracing::warn!(
                                            hook_id = %hook_id,
                                            event_type = %event_type,
                                            failure = %error,
                                            "Timed-out Hook handler failed while settling"
                                        );
                                    }
                                }
                            });
                            if let Err(error) = dispatcher.dispatch("hook.timeout-settle", settle) {
                                tracing::warn!(
                                    hook_id = %hook.id,
                                    event_type = %event.event_type(),
                                    failure = %error,
                                    "Timed-out Hook handler could not be supervised"
                                );
                            }
                        }
                        self.handler_failure(
                            hook,
                            event,
                            format!("handler timed out after {} ms", hook.config.timeout_ms),
                        )
                    }
                }
            }
            // Hooks may be registered only to select events for an SDK listener.
            // Without an actual handler there is no gating policy to fail.
            None => HookOutcome::Continue(None),
        }
    }

    /// Events whose result gates a protected operation.
    ///
    /// These are the hook points whose callers explicitly consume a block
    /// decision before producing tool or planning side effects. Other hook
    /// points are observational or advisory and remain best-effort.
    fn is_gating_event(event: &HookEvent) -> bool {
        matches!(
            event,
            HookEvent::PreToolUse(_)
                | HookEvent::PermissionRequest(_)
                | HookEvent::PreCompact(_)
                | HookEvent::PrePrompt(_)
                | HookEvent::PrePlanning(_)
                | HookEvent::PreRunControl(_)
        )
    }

    /// Map handler infrastructure failures according to the hook point's role.
    fn handler_failure(&self, hook: &Hook, event: &HookEvent, failure: String) -> HookOutcome {
        tracing::warn!(
            hook_id = %hook.id,
            event_type = %event.event_type(),
            failure = %failure,
            gating = Self::is_gating_event(event),
            "Hook handler failed"
        );

        if Self::is_gating_event(event) {
            HookOutcome::Block {
                reason: format!("Required hook '{}' failed: {}", hook.id, failure),
            }
        } else {
            HookOutcome::Continue(None)
        }
    }

    /// Convert HookResponse to the lossless internal outcome.
    fn response_to_outcome(&self, response: HookResponse) -> HookOutcome {
        match response.action {
            HookAction::Continue => HookOutcome::Continue(response.modified),
            HookAction::Block => HookOutcome::Block {
                reason: response.reason.unwrap_or_else(|| "Blocked".to_string()),
            },
            HookAction::Retry => HookOutcome::Retry {
                reason: response
                    .reason
                    .unwrap_or_else(|| "Hook requested a retry".to_string()),
                retry_after_ms: response.retry_delay_ms.unwrap_or(1000),
            },
            HookAction::Skip => HookOutcome::Skip,
        }
    }

    /// Get the number of registered hooks
    pub fn hook_count(&self) -> usize {
        read_or_recover(&self.hooks).len()
    }

    /// Get a hook by ID
    pub fn get_hook(&self, id: &str) -> Option<Hook> {
        read_or_recover(&self.hooks)
            .get(id)
            .map(|hook| (**hook).clone())
    }

    /// Get all hooks
    pub fn all_hooks(&self) -> Vec<Hook> {
        read_or_recover(&self.hooks)
            .values()
            .map(|hook| (**hook).clone())
            .collect()
    }
}

// Implement HookExecutor trait for HookEngine
#[async_trait]
impl HookExecutor for HookEngine {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        HookEngine::fire(self, event).await
    }

    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        HookEngine::fire_outcome(self, event).await
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
