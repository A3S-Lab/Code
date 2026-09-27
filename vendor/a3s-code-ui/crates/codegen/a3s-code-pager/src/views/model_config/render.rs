//! Draw the provider/model modal: providers on the left, connection and models on the right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::state::{DetailField, Focus, ModelConfigState};
use crate::theme::Theme;
use crate::views::modal_window::{self, ModalSizing, ModalWindowConfig, Shortcut};

pub(crate) const MODAL_TITLE: &str = "Models";

const PROVIDER_SHORTCUTS: [Shortcut; 5] = [
    Shortcut {
        label: "a add provider",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "enter models",
        clickable: false,
        id: 1,
    },
    Shortcut {
        label: "x remove",
        clickable: false,
        id: 2,
    },
    Shortcut {
        label: "s save",
        clickable: false,
        id: 3,
    },
    Shortcut {
        label: "esc close",
        clickable: false,
        id: 4,
    },
];

const MODEL_SHORTCUTS: [Shortcut; 6] = [
    Shortcut {
        label: "a add model",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "enter edit",
        clickable: false,
        id: 1,
    },
    Shortcut {
        label: "x remove",
        clickable: false,
        id: 2,
    },
    Shortcut {
        label: "space tools",
        clickable: false,
        id: 3,
    },
    Shortcut {
        label: "s save",
        clickable: false,
        id: 4,
    },
    Shortcut {
        label: "esc close",
        clickable: false,
        id: 5,
    },
];

pub(crate) fn render_model_config(
    buf: &mut Buffer,
    area: Rect,
    state: &mut ModelConfigState,
    compact: bool,
) {
    let theme = Theme::current();
    let shortcuts: &[Shortcut] = if state.focus == Focus::Providers {
        &PROVIDER_SHORTCUTS
    } else {
        &MODEL_SHORTCUTS
    };
    let config = ModalWindowConfig {
        title: MODAL_TITLE,
        tabs: None,
        shortcuts,
        sizing: ModalSizing::large().with_compact(compact),
        fold_info: None,
    };
    let Some(content) =
        modal_window::render_modal_window(buf, area, &mut state.window, &config, &theme)
    else {
        return;
    };
    paint(buf, content.content, state);
}

fn paint(buf: &mut Buffer, area: Rect, state: &ModelConfigState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let normal = Style::default();
    let dim = Style::default().add_modifier(Modifier::DIM);
    let selected = Style::default().add_modifier(Modifier::REVERSED);
    let header = Style::default().add_modifier(Modifier::BOLD);
    buf.set_stringn(area.x, area.y, &state.status, area.width as usize, dim);
    if area.height < 3 {
        return;
    }
    let body = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height - 1,
    };
    let left_width = (body.width / 3)
        .clamp(18, 32)
        .min(body.width.saturating_sub(20));
    let left = Rect {
        x: body.x,
        y: body.y,
        width: left_width,
        height: body.height,
    };
    let right = Rect {
        x: body.x + left_width,
        y: body.y,
        width: body.width.saturating_sub(left_width),
        height: body.height,
    };
    buf.set_stringn(left.x, left.y, "Providers", left.width as usize, header);
    let mut row = 1u16;
    for (index, provider) in state.draft.providers.iter().enumerate() {
        if row >= left.height {
            break;
        }
        let marker = if state.focus == Focus::Providers && state.provider_index == index {
            ">"
        } else {
            " "
        };
        let style = if state.focus == Focus::Providers && state.provider_index == index {
            selected
        } else {
            normal
        };
        let models = provider.models.len();
        let line = format!(
            "{marker} {}  {models} {}",
            provider.id,
            if models == 1 { "model" } else { "models" }
        );
        buf.set_stringn(left.x, left.y + row, &line, left.width as usize, style);
        row += 1;
    }
    if row < left.height {
        let on_add =
            state.focus == Focus::Providers && state.provider_index == state.draft.providers.len();
        buf.set_stringn(
            left.x,
            left.y + row,
            if on_add {
                "> + Add provider"
            } else {
                "  + Add provider"
            },
            left.width as usize,
            if on_add { selected } else { dim },
        );
    }

    let mut line_y = right.y;
    let title = state
        .selected_provider()
        .map(|provider| format!("Connection  {}", provider.id))
        .unwrap_or_else(|| "Connection".to_string());
    buf.set_stringn(right.x, line_y, &title, right.width as usize, header);
    line_y += 1;
    let fields = state.detail_fields();
    let mut saw_models = false;
    let mut saw_session = false;
    for (index, field) in fields.iter().enumerate() {
        if line_y >= right.y + right.height {
            break;
        }
        if !saw_models && is_model_section(*field) {
            let count = state
                .selected_provider()
                .map(|provider| provider.models.len())
                .unwrap_or(0);
            let heading = state
                .selected_provider()
                .map(|provider| format!("Models in {} ({count})   a add", provider.id))
                .unwrap_or_else(|| "Models".to_string());
            buf.set_stringn(right.x, line_y, &heading, right.width as usize, header);
            line_y += 1;
            saw_models = true;
            if count == 0 && line_y < right.y + right.height {
                buf.set_stringn(
                    right.x,
                    line_y,
                    "  no models yet",
                    right.width as usize,
                    dim,
                );
                line_y += 1;
            }
        }
        if !saw_session && is_session(*field) && line_y < right.y + right.height {
            buf.set_stringn(right.x, line_y, "Default", right.width as usize, header);
            line_y += 1;
            saw_session = true;
        }
        if line_y >= right.y + right.height {
            break;
        }
        let focused = state.focus == Focus::Detail && state.detail_index == index;
        let style = if focused { selected } else { normal };
        let text = field_line(state, *field, focused);
        let shown = if focused {
            format!("> {text}")
        } else {
            format!("  {text}")
        };
        buf.set_stringn(right.x, line_y, &shown, right.width as usize, style);
        line_y += 1;
    }
}

fn is_model_section(field: DetailField) -> bool {
    matches!(field, DetailField::ModelSummary(_) | DetailField::AddModel)
}

fn is_session(field: DetailField) -> bool {
    matches!(
        field,
        DetailField::DefaultModel
            | DetailField::ThinkingBudget
            | DetailField::TimeoutSeconds
            | DetailField::DefaultProvider
    )
}

fn field_line(state: &ModelConfigState, field: DetailField, focused: bool) -> String {
    let edited = focused
        .then(|| state.editing_text())
        .flatten()
        .map(str::to_string);
    let value = |field: DetailField| state.field_text(field).unwrap_or_default();
    let shown = |fallback: String| edited.clone().unwrap_or(fallback);
    match field {
        DetailField::ProviderId => format!("Provider   {}", shown(value(field))),
        DetailField::BaseUrl => format!("API URL    {}", blank(&shown(value(field)), "not set")),
        DetailField::ApiKey => {
            let raw = shown(value(field));
            let masked = if state.reveal_key || edited.is_some() {
                blank(&raw, "not set")
            } else if raw.is_empty() {
                "not set".to_string()
            } else {
                "••••••••".to_string()
            };
            format!("API key    {masked}")
        }
        DetailField::ModelSummary(index) => model_summary(state, index),
        DetailField::ModelId(_) => format!("    id       {}", shown(value(field))),
        DetailField::ModelName(_) => format!("    name     {}", shown(value(field))),
        DetailField::ModelBaseUrl(_) => {
            connection_line("API URL", &shown(value(field)), edited.is_some(), false)
        }
        DetailField::ModelApiKey(_) => connection_line(
            "API key",
            &shown(value(field)),
            edited.is_some() || state.reveal_key,
            true,
        ),
        DetailField::ModelContext(_) => format!("    context  {}", shown(value(field))),
        DetailField::ModelOutput(_) => format!("    output   {}", shown(value(field))),
        DetailField::AddModel => "+ Add model".to_string(),
        DetailField::DefaultProvider => {
            format!("Provider   {}", blank(&shown(value(field)), "not set"))
        }
        DetailField::DefaultModel => {
            format!("Model      {}", blank(&shown(value(field)), "not set"))
        }
        DetailField::ThinkingBudget => format!(
            "Reasoning  {} tokens",
            blank(
                &shown(state.draft.thinking_budget.clone()),
                "runtime default"
            )
        ),
        DetailField::TimeoutSeconds => format!(
            "Timeout    {} sec",
            blank(
                &shown(state.draft.timeout_seconds.clone()),
                "runtime default"
            )
        ),
    }
}

fn connection_line(label: &str, value: &str, visible: bool, secret: bool) -> String {
    if value.is_empty() {
        return format!("    {label:<8} inherit");
    }
    if secret && !visible {
        return format!("    {label:<8} ••••••••");
    }
    format!("    {label:<8} {value}")
}

fn model_summary(state: &ModelConfigState, index: usize) -> String {
    let Some(provider) = state.selected_provider() else {
        return "model".to_string();
    };
    let Some(model) = provider.models.get(index) else {
        return "model".to_string();
    };
    let tools = if model.tool_call {
        "tools on"
    } else {
        "tools off"
    };
    let catalog_id = format!("{}/{}", provider.id, model.id);
    let default_mark = if state.draft.default_model == catalog_id {
        "  default"
    } else {
        ""
    };
    format!(
        "{}. {}   {}   {tools}{default_mark}",
        index + 1,
        model.id,
        model.name
    )
}

fn blank(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}
