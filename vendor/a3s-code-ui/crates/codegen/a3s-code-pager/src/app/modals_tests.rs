use std::sync::Arc;

use agent_client_protocol as acp;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::actions::ActionRegistry;
use crate::app::agent_view::AgentPane;
use crate::app::agent_view::test_fixtures::make_agent;
use crate::views::modal::ActiveModal;

/// Ctrl+M, then Enter on a reasoning model: the effort sub-menu opens on the model's default effort row.
#[test]
fn arg_picker_effort_phase_opens_on_default_row() {
    let mut agent = make_agent();
    let id = acp::ModelId::new(Arc::from("reasoning-x"));
    agent.session.models.available.insert(
        id.clone(),
        acp::ModelInfo::new(id, "Reasoning X").meta(
            serde_json::json!({ "supportsReasoningEffort": true, "reasoningEffort": "high" })
                .as_object()
                .cloned(),
        ),
    );
    // Ctrl+M is the multiline toggle while the prompt is focused; the picker binding lives on the agent screen
    agent.set_active_pane(AgentPane::Scrollback, true);

    let registry = ActionRegistry::defaults();
    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL)),
        &registry,
    );
    assert!(
        matches!(
            agent.active_modal.as_ref(),
            Some(ActiveModal::ArgPicker { command, args_query, .. })
                if command == "model" && args_query.is_empty()
        ),
        "Ctrl+M must open the /model picker in the model phase"
    );

    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &registry,
    );
    let Some(ActiveModal::ArgPicker {
        args_query,
        items,
        state,
        ..
    }) = agent.active_modal.as_ref()
    else {
        panic!("expected the /model picker to chain into the effort phase");
    };
    assert_eq!("Reasoning X ", args_query);
    assert_eq!(1, state.selected);
    assert_eq!(
        Some("Reasoning X high"),
        items.get(1).map(|item| item.insert_text.as_str())
    );
}

fn plain_model(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
    let id = acp::ModelId::new(Arc::from(id));
    (id.clone(), acp::ModelInfo::new(id, name))
}

/// Ctrl+M opens the live `/model` popup. With an empty filter, Left/Right must
/// change the provider tab and the rows, not just move a cursor.
#[test]
fn model_picker_arrows_switch_provider_and_filter_rows() {
    let mut agent = make_agent();
    let (cc, cc_info) = plain_model("cc-switch/glm", "cc-switch · glm");
    let (grok, grok_info) = plain_model("grok/grok-4", "grok · grok-4");
    agent.session.models.available.insert(cc, cc_info);
    agent
        .session
        .models
        .available
        .insert(grok.clone(), grok_info);
    agent.session.models.current = Some(grok);
    agent.set_active_pane(AgentPane::Scrollback, true);

    let registry = ActionRegistry::defaults();
    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL)),
        &registry,
    );

    let Some(ActiveModal::ArgPicker {
        command,
        items,
        window,
        ..
    }) = agent.active_modal.as_ref()
    else {
        panic!("Ctrl+M must open the model picker");
    };
    assert_eq!(command, "model");
    assert_eq!(
        window.active_tab, 1,
        "current grok model opens the grok tab"
    );
    assert!(
        items.iter().all(|item| item.match_text.contains("grok")),
        "opening tab must list only the current provider: {items:?}"
    );

    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
        &registry,
    );
    let Some(ActiveModal::ArgPicker { items, window, .. }) = agent.active_modal.as_ref() else {
        panic!("picker must stay open after Left");
    };
    assert_eq!(window.active_tab, 0);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].match_text.contains("cc-switch"),
        "Left from grok wraps to cc-switch, got {:?}",
        items[0].match_text
    );

    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        &registry,
    );
    let Some(ActiveModal::ArgPicker { items, window, .. }) = agent.active_modal.as_ref() else {
        panic!("picker must stay open after Right");
    };
    assert_eq!(window.active_tab, 1);
    assert!(items.iter().all(|item| item.match_text.contains("grok")));
}

/// The effort phase must offer the a3s budget levels advertised on model meta,
/// with `high` highlighted when that level is the model default.
#[test]
fn model_picker_effort_phase_lists_a3s_budget_levels() {
    let mut agent = make_agent();
    let id = acp::ModelId::new(Arc::from("cc-switch/glm"));
    let efforts = ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .map(|level| {
            serde_json::json!({
                "value": level,
                "id": level,
                "label": level,
                "default": level == "high",
            })
        })
        .collect::<Vec<_>>();
    agent.session.models.available.insert(
        id.clone(),
        acp::ModelInfo::new(id, "cc-switch · glm").meta(
            serde_json::json!({
                "supportsReasoningEffort": true,
                "reasoningEffort": "high",
                "reasoningEfforts": efforts,
            })
            .as_object()
            .cloned(),
        ),
    );
    agent.set_active_pane(AgentPane::Scrollback, true);

    let registry = ActionRegistry::defaults();
    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL)),
        &registry,
    );
    agent.handle_input(
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &registry,
    );

    let Some(ActiveModal::ArgPicker { items, state, .. }) = agent.active_modal.as_ref() else {
        panic!("Enter on a reasoning model must open the effort phase");
    };
    let shown: Vec<&str> = items.iter().map(|item| item.display.as_str()).collect();
    assert_eq!(shown, ["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(state.selected, 2, "high is the advertised default");
    assert_eq!(items[2].insert_text, "cc-switch · glm high");
}
