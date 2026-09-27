use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use crate::app::dispatch::settings::ui::refresh_open_settings_modals;
use crate::settings::SettingValue;

pub(in crate::app::dispatch) fn set_dashboard_preview(
    app: &mut AppView,
    enabled: bool,
) -> Vec<Effect> {
    let previous = app.current_ui.dashboard_preview_enabled();
    if previous == enabled {
        return Vec::new();
    }

    crate::app::dispatch::dashboard::ensure_dashboard_state(app);
    app.current_ui.dashboard_preview = Some(enabled);
    refresh_open_settings_modals(app);
    tracing::info!(target: "settings", key = "dashboard_preview", value = enabled, "setting changed");

    vec![Effect::PersistSetting {
        key: "dashboard_preview",
        value: SettingValue::Bool(enabled),
        rollback_value: SettingValue::Bool(previous),
    }]
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
