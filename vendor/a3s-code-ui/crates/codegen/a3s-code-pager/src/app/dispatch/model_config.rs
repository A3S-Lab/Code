//! Open the `/config` provider and model modal.
//!
//! The ACL snapshot is loaded before dispatch. This module only mounts it.

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};
use crate::app::dispatch::ctx::{SwitchCause, switch_to_agent};
use crate::views::modal::ActiveModal;
use crate::views::model_config::ModelConfigState;

pub(in crate::app::dispatch) fn dispatch_open_model_config(
    app: &mut AppView,
    state: Box<ModelConfigState>,
) -> Vec<Effect> {
    if let ActiveView::Agent(id) = app.active_view {
        if let Some(agent) = app.agents.get_mut(&id) {
            if matches!(agent.active_modal, Some(ActiveModal::ModelConfig { .. })) {
                agent.active_modal = None;
                return Vec::new();
            }
        }
    }

    let mut effects = Vec::new();
    let id = match app.active_view {
        ActiveView::Agent(id) => id,
        _ => {
            if let Some(existing) = app.agents.keys().next().copied() {
                switch_to_agent(app, existing, SwitchCause::Picker);
                existing
            } else {
                let (new_id, create_effects) =
                    crate::app::dispatch::session::lifecycle::dispatch_new_session_inner_with_id(
                        app, None, false,
                    );
                effects.extend(create_effects);
                new_id
            }
        }
    };
    if let Some(agent) = app.agents.get_mut(&id) {
        agent.active_modal = Some(ActiveModal::ModelConfig { state });
    }
    effects
}
