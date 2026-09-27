use super::*;
use crate::app::actions::{Action, TaskResult};
use crate::app::agent::AgentId;
use crate::app::app_view::ActiveView;
use crate::app::dispatch::dashboard::ensure_dashboard_state;
use crate::app::dispatch::router::dispatch;
use crate::app::dispatch::settings::ui::action_for_reset;
use crate::app::dispatch::task_result::dispatch_task_result;
use crate::app::dispatch::tests::test_app_with_agent;
use crate::views::dashboard::DashboardRowId;
use crate::views::dashboard::peek::{PeekFields, PeekPanelState};
use crate::views::dashboard::state::DeferredPeekSend;
use crate::views::modal::ActiveModal;

#[test]
fn preview_setting_updates_existing_and_new_dashboards_and_resets() {
    let mut app = test_app_with_agent();
    app.workspace_dashboard_enabled = true;
    ensure_dashboard_state(&mut app);
    let _ = dispatch(Action::OpenSettings, &mut app);

    let effects = dispatch(Action::SetDashboardPreview(false), &mut app);

    assert!(matches!(
        effects.as_slice(),
        [Effect::PersistSetting {
            key: "dashboard_preview",
            value: SettingValue::Bool(false),
            rollback_value: SettingValue::Bool(true),
        }]
    ));
    assert!(!app.current_ui.dashboard_preview_enabled());
    assert!(app.dashboard.as_ref().expect("dashboard").preview_enabled);
    let Some(ActiveModal::Settings { state }) = app
        .agents
        .get(&AgentId(0))
        .and_then(|a| a.active_modal.as_ref())
    else {
        panic!("settings modal");
    };
    assert!(!state.ui_snapshot.dashboard_preview_enabled());

    assert!(dispatch(Action::SetDashboardPreview(false), &mut app).is_empty());

    let _ = dispatch_task_result(
        TaskResult::SettingPersisted {
            key: "dashboard_preview",
            value: SettingValue::Bool(false),
        },
        &mut app,
    );
    assert!(!app.dashboard.as_ref().expect("dashboard").preview_enabled);

    app.dashboard = None;

    ensure_dashboard_state(&mut app);

    assert!(
        !app.dashboard
            .as_ref()
            .expect("reopened dashboard")
            .preview_enabled
    );

    let action =
        action_for_reset("dashboard_preview", &SettingValue::Bool(true)).expect("reset action");

    let _ = dispatch(action, &mut app);

    assert!(app.current_ui.dashboard_preview_enabled());
    assert!(!app.dashboard.as_ref().expect("dashboard").preview_enabled);

    let _ = dispatch_task_result(
        TaskResult::SettingPersisted {
            key: "dashboard_preview",
            value: SettingValue::Bool(true),
        },
        &mut app,
    );
    assert!(app.dashboard.as_ref().expect("dashboard").preview_enabled);
}

#[test]
fn first_dashboard_open_uses_previous_preference_until_save_finishes() {
    let mut app = test_app_with_agent();
    app.workspace_dashboard_enabled = true;
    assert!(app.dashboard.is_none());

    let _ = dispatch(Action::SetDashboardPreview(false), &mut app);
    ensure_dashboard_state(&mut app);

    assert!(!app.current_ui.dashboard_preview_enabled());
    assert!(app.dashboard.as_ref().expect("dashboard").preview_enabled);

    let _ = dispatch_task_result(
        TaskResult::SettingPersistFailed {
            key: "dashboard_preview",
            error: "read only".to_owned(),
            rollback_value: SettingValue::Bool(true),
        },
        &mut app,
    );

    assert!(app.current_ui.dashboard_preview_enabled());
    assert!(app.dashboard.as_ref().expect("dashboard").preview_enabled);
}

#[test]
fn failed_preview_write_restores_the_previous_setting() {
    let mut app = test_app_with_agent();
    app.workspace_dashboard_enabled = true;
    ensure_dashboard_state(&mut app);
    let _ = dispatch(Action::SetDashboardPreview(false), &mut app);

    let _ = dispatch_task_result(
        TaskResult::SettingPersistFailed {
            key: "dashboard_preview",
            error: "read only".to_owned(),
            rollback_value: SettingValue::Bool(true),
        },
        &mut app,
    );

    assert!(app.current_ui.dashboard_preview_enabled());
    assert!(app.dashboard.as_ref().expect("dashboard").preview_enabled);
}

fn app_with_preview_draft(question: Option<&str>) -> AppView {
    let mut app = test_app_with_agent();
    app.workspace_dashboard_enabled = true;
    ensure_dashboard_state(&mut app);
    let dashboard = app.dashboard.as_mut().expect("dashboard");
    let row = DashboardRowId::TopLevel(AgentId(0));
    dashboard.focus_row(row.clone());
    dashboard.set_peek(Some(PeekPanelState::new(
        row.clone(),
        PeekFields {
            label: "Session".to_owned(),
            time_ago: String::new(),
            response_type: "Idle".to_owned(),
            last_user_message: None,
            question: question.map(str::to_owned),
            options: question
                .map(|_| vec![("reject".to_owned(), "Reject".to_owned())])
                .unwrap_or_default(),
            request_id: question.map(|_| 1),
            reject_option: question.map(|_| 0),
        },
    )));
    if let Some(panel) = dashboard.peek.as_mut() {
        panel.selected_option = question.map(|_| 0);
    }
    dashboard.begin_peek_viewport(row.clone(), &mut app.agents);
    dashboard.peek_reply.handle_paste("unsent text");
    dashboard.peek_reply_rect = Some(ratatui::layout::Rect::new(0, 30, 80, 1));
    if question.is_none() {
        dashboard.deferred_peek_send = Some(DeferredPeekSend { row, attach: false });
    }
    app
}

#[test]
fn failed_preview_disable_preserves_reply_and_permission_drafts() {
    for question in [None, Some("Allow command?")] {
        let mut app = app_with_preview_draft(question);
        let _ = dispatch(Action::OpenSettings, &mut app);
        let _ = dispatch(Action::SetDashboardPreview(false), &mut app);

        let dashboard = app.dashboard.as_ref().expect("dashboard");
        assert!(dashboard.preview_enabled);
        assert_eq!("unsent text", dashboard.peek_reply.text());
        assert!(dashboard.peek_reply.textarea.can_undo());
        assert!(app.agents.get(&AgentId(0)).expect("agent").toast.is_none());

        let _ = dispatch_task_result(
            TaskResult::SettingPersistFailed {
                key: "dashboard_preview",
                rollback_value: SettingValue::Bool(true),
                error: "read only".to_owned(),
            },
            &mut app,
        );

        assert!(app.current_ui.dashboard_preview_enabled());
        let agent = app.agents.get(&AgentId(0)).expect("agent");
        let Some(ActiveModal::Settings { state }) = agent.active_modal.as_ref() else {
            panic!("settings modal");
        };
        assert!(state.ui_snapshot.dashboard_preview_enabled());
        assert!(
            agent
                .toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains("Could not save dashboard_preview"))
        );
        let dashboard = app.dashboard.as_mut().expect("dashboard");
        assert!(dashboard.preview_enabled);
        let panel = dashboard.peek.as_ref().expect("preview remains open");
        assert_eq!(question, panel.question.as_deref());
        assert_eq!(question.map(|_| 0), panel.selected_option);
        assert_eq!("unsent text", dashboard.peek_reply.text());
        assert!(dashboard.peek_viewport.is_some());
        assert!(dashboard.peek_reply_rect.is_some());
        assert_eq!(question.is_none(), dashboard.deferred_peek_send.is_some());
        assert!(dashboard.peek_reply.textarea.undo());
        assert!(dashboard.peek_reply.text().is_empty());
    }
}

#[test]
fn successful_preview_disable_clears_draft_only_after_confirmation() {
    let mut app = app_with_preview_draft(None);
    let _ = dispatch(Action::SetDashboardPreview(false), &mut app);
    assert_eq!(
        "unsent text",
        app.dashboard.as_ref().expect("dashboard").peek_reply.text()
    );

    let _ = dispatch_task_result(
        TaskResult::SettingPersisted {
            key: "show_timestamps",
            value: SettingValue::Bool(false),
        },
        &mut app,
    );
    assert!(app.dashboard.as_ref().expect("dashboard").peek.is_some());

    let _ = dispatch_task_result(
        TaskResult::SettingPersisted {
            key: "dashboard_preview",
            value: SettingValue::Bool(false),
        },
        &mut app,
    );

    let dashboard = app.dashboard.as_ref().expect("dashboard");
    assert!(!dashboard.preview_enabled);
    assert!(dashboard.peek.is_none());
    assert!(dashboard.peek_viewport.is_none());
    assert!(dashboard.peek_reply_rect.is_none());
    assert!(dashboard.deferred_peek_send.is_none());
    assert!(dashboard.peek_reply.text().is_empty());
    assert!(!dashboard.peek_reply.textarea.can_undo());
}

#[test]
fn superseded_disable_confirmation_does_not_clear_preview_draft() {
    let mut app = app_with_preview_draft(None);
    let _ = dispatch(Action::SetDashboardPreview(false), &mut app);
    let _ = dispatch(Action::SetDashboardPreview(true), &mut app);

    let _ = dispatch_task_result(
        TaskResult::SettingPersisted {
            key: "dashboard_preview",
            value: SettingValue::Bool(false),
        },
        &mut app,
    );

    let dashboard = app.dashboard.as_ref().expect("dashboard");
    assert!(app.current_ui.dashboard_preview_enabled());
    assert!(dashboard.preview_enabled);
    assert!(dashboard.peek.is_some());
    assert_eq!("unsent text", dashboard.peek_reply.text());
}

#[test]
fn loaded_preference_survives_dashboard_recreation() {
    let mut app = test_app_with_agent();
    app.workspace_dashboard_enabled = true;
    app.current_ui = toml::from_str("dashboard_preview = false").expect("saved UI config");
    ensure_dashboard_state(&mut app);
    let dashboard = app.dashboard.as_mut().expect("dashboard");
    dashboard.focus_row(DashboardRowId::TopLevel(AgentId(0)));

    let _ =
        dashboard.layout_with_preview(ratatui::layout::Rect::new(0, 0, 100, 40), &mut app.agents);

    assert!(dashboard.peek.is_none());

    app.active_view = ActiveView::Agent(AgentId(0));
    app.dashboard = None;

    ensure_dashboard_state(&mut app);

    assert!(!app.dashboard.as_ref().expect("dashboard").preview_enabled);
}
