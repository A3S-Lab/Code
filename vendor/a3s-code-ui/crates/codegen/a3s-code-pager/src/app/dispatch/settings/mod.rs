pub(in crate::app::dispatch) mod dashboard;
pub(in crate::app::dispatch) mod setters;
pub(in crate::app::dispatch) mod ui;

use super::status::toast_persist_failure;
use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use crate::app::dispatch::settings::ui::save_success_toast;
use crate::settings::SettingValue;
use a3s_code_shell::agent::config::Feature;

pub(in crate::app::dispatch) fn handle_setting_persisted(
    app: &mut AppView,
    key: &str,
    value: SettingValue,
) {
    if let ("dashboard_preview", SettingValue::Bool(enabled)) = (key, value) {
        // An older save must not close a preview the user has re-enabled.
        if app.current_ui.dashboard_preview_enabled() != enabled {
            return;
        }
        if let Some(dashboard) = app.dashboard.as_mut() {
            dashboard.set_preview_enabled(enabled, &mut app.agents);
        }
        app.show_toast(&save_success_toast("Dashboard preview", enabled));
    }
}

/// One `[features]` write finished. The row that issued it settles and may issue the intent queued behind it;
/// `subagent_model_inheritance` is the only row today.
pub(in crate::app::dispatch) fn handle_feature_override_persisted(
    app: &mut AppView,
    feature: Feature,
    result: Result<Option<bool>, String>,
) -> Vec<Effect> {
    let persisted = match result {
        Ok(saved) => Some(saved),
        Err(error) => {
            tracing::warn!(target: "settings", key = feature.key(), %error, "setting persist failed");
            toast_persist_failure(app, feature.key(), &error);
            None
        }
    };
    let effects = if feature == app.subagent_model_inheritance.feature {
        setters::settle_subagent_model_inheritance_write(app, persisted)
    } else {
        vec![]
    };
    ui::refresh_open_settings_modals(app);
    effects
}
