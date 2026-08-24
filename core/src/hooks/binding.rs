use std::fmt;
use std::sync::Arc;

use super::{Hook, HookEventType, HookHandler};

/// Immutable Hook definition and its exact executable handler.
///
/// A projected Hook must carry both halves as one value so publication cannot
/// pair metadata from one generation with a handler from another.
pub struct HookBinding {
    hook: Arc<Hook>,
    handler: Arc<dyn HookHandler>,
}

impl HookBinding {
    pub fn new(hook: Hook, handler: Arc<dyn HookHandler>) -> Self {
        Self {
            hook: Arc::new(hook),
            handler,
        }
    }

    pub fn hook(&self) -> &Hook {
        &self.hook
    }

    pub fn handler(&self) -> &dyn HookHandler {
        self.handler.as_ref()
    }

    pub(crate) fn hook_arc(&self) -> &Arc<Hook> {
        &self.hook
    }

    pub(crate) fn handler_arc(&self) -> &Arc<dyn HookHandler> {
        &self.handler
    }

    pub(crate) fn validate_run_scope(&self) -> Result<(), &'static str> {
        match self.hook.event_type {
            HookEventType::SessionStart | HookEventType::SessionEnd => {
                Err("Session lifecycle Hook events are outside Run-scoped capability projection")
            }
            HookEventType::SkillLoad | HookEventType::SkillUnload => {
                Err("Skill lifecycle Hook events have no Run-scoped production emitter")
            }
            _ => Ok(()),
        }
    }
}

impl fmt::Debug for HookBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookBinding")
            .field("hook", &self.hook)
            .finish_non_exhaustive()
    }
}
