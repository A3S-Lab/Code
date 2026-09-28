//! `/model` (alias `/m`): switch the model and optionally its reasoning effort.
//! Chained autocomplete: after picking a reasoning-supported model, the trailing space re-opens the dropdown into a `low|medium|high|xhigh` sub-menu.

use a3s_code_shell::sampling::types::{ReasoningEffortOption, supports_reasoning_effort_meta};
use agent_client_protocol as acp;

use crate::acp::model_state::ModelState;
use crate::app::actions::Action;
use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};
use crate::slash::commands::effort_levels::build_effort_arg_items;

/// Switch the active model (and optionally its reasoning effort).
pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    slash_meta! {
        name: "model",
        aliases: ["m"],
        description: "Switch the active model",
        usage: "/model <name> [effort]",
        takes_args: true,
        args_required: true,
        session_scoped: true,
        // The dashboard offers `/model` to pick the model for the next spawned agent (intercepted in `dispatch_dashboard_dispatch_slash`).
        offered_when_session_less: true,
        arg_placeholder: "<model> [effort]",
    }

    fn suggest_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }

        // Effort phase if input is "<reasoning-model> ", else model phase.
        if let Some((model_id, prefix)) = matched_reasoning_prefix(ctx.models, args_query) {
            return Some(build_effort_items(ctx.models, &model_id, &prefix));
        }
        Some(build_model_items(ctx.models))
    }

    fn preselected_arg(&self, ctx: &AppCtx, args_query: &str) -> Option<String> {
        let (model_id, prefix) = matched_reasoning_prefix(ctx.models, args_query)?;
        // A typed effort filter hands the opening row to the match ranking.
        if !args_query.trim_end().eq_ignore_ascii_case(&prefix) {
            return None;
        }
        let option = ctx.models.preselected_effort_option_for(&model_id)?;
        Some(effort_insert_text(&prefix, &option))
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Error("Usage: /model <name> [effort]".into());
        }

        // Prefer an exact full-string catalog match first. Model display names often contain spaces ("A3S Code 4.5").
        // If we split on the last token first, a shorter catalog entry ("A3S Code") would steal the prefix and treat "4.5" as an effort level
        if let Some(id) = ctx.models.resolve_by_name_or_id(trimmed) {
            return CommandResult::Action(Action::SetDefaultModel(id));
        }

        // Trailing effort on a reasoning model is a session switch. The token keeps its spaces.
        if let Some((id, token)) = split_model_effort(ctx.models, trimmed) {
            return match ctx.models.resolve_effort_for_model(&id, token) {
                Ok(effort) => CommandResult::Action(Action::SwitchModel {
                    model_id: id,
                    effort: Some(effort),
                }),
                Err(err) => CommandResult::Error(err.message()),
            };
        }

        CommandResult::Error(format!("Unknown model: {trimmed}"))
    }
}

fn supports_reasoning_effort(info: &acp::ModelInfo) -> bool {
    supports_reasoning_effort_meta(info.meta.as_ref())
}

fn split_on_model_key<'a>(args: &'a str, key: &str) -> Option<&'a str> {
    let rest = args.get(key.len()..)?;
    if args
        .get(..key.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(key))
        && rest.starts_with(char::is_whitespace)
    {
        Some(rest)
    } else {
        None
    }
}

/// Longest reasoning-model name or id that prefixes `args`, and the text after it.
fn longest_reasoning_prefix<'a>(
    models: &'a ModelState,
    args: &'a str,
) -> Option<(&'a acp::ModelId, &'a str, &'a str)> {
    let mut best: Option<(&acp::ModelId, &str, &str)> = None;
    for (id, info) in models
        .available
        .iter()
        .filter(|(_, info)| supports_reasoning_effort(info))
    {
        let name = info.name.as_str();
        let id_str = id.0.as_ref();
        for key in [name, id_str] {
            if best.is_some_and(|(_, prev, _)| prev.len() >= key.len()) {
                continue;
            }
            if let Some(rest) = split_on_model_key(args, key) {
                best = Some((id, key, rest));
            }
        }
    }
    best
}

fn split_model_effort<'a>(
    models: &'a ModelState,
    args: &'a str,
) -> Option<(acp::ModelId, &'a str)> {
    let (id, _, rest) = longest_reasoning_prefix(models, args)?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some((id.clone(), token))
    }
}

fn matched_reasoning_prefix(
    models: &ModelState,
    args_query: &str,
) -> Option<(acp::ModelId, String)> {
    let (id, key, _) = longest_reasoning_prefix(models, args_query)?;
    Some((id.clone(), key.to_string()))
}

/// True when `/model` args are in the effort submenu (`"<model> "`), not the provider model list.
pub(crate) fn model_args_in_effort_phase(models: &ModelState, args_query: &str) -> bool {
    matched_reasoning_prefix(models, args_query).is_some()
}

/// One row per logical model.
/// Reasoning models get a trailing space in `insert_text` so the prompt widget chains into the effort sub-menu.
fn build_model_items(models: &ModelState) -> Vec<ArgItem> {
    let current_id = models.current.as_ref();
    let mut items: Vec<ArgItem> = Vec::with_capacity(models.available.len());
    for (id, info) in &models.available {
        let is_current = current_id == Some(id);
        let supports = supports_reasoning_effort(info);

        let display = if is_current {
            format!("{} (current)", info.name)
        } else {
            info.name.clone()
        };

        // A trailing space on reasoning models signals "more input expected" to the prompt widget
        // Enter then advances to the effort phase instead of submitting
        let insert_text = if supports {
            format!("{} ", info.name)
        } else {
            info.name.clone()
        };

        items.push(ArgItem {
            display,
            match_text: info.name.clone(),
            insert_text,
            description: info.description.clone().unwrap_or_default(),
        });
    }
    items
}

/// One row per effort level for the `/model` chained effort phase.
/// `prefix` is the name or catalog id the user typed. `insert_text` is `"{prefix} {effort}"`.
fn build_effort_items(models: &ModelState, model_id: &acp::ModelId, prefix: &str) -> Vec<ArgItem> {
    if !models.available.contains_key(model_id) {
        return Vec::new();
    }
    let is_current_model = models.current.as_ref() == Some(model_id);
    let options = models.reasoning_effort_options_for(model_id);
    build_effort_arg_items(
        &options,
        models.reasoning_effort,
        is_current_model,
        |option| effort_insert_text(prefix, option),
    )
}

fn effort_insert_text(prefix: &str, option: &ReasoningEffortOption) -> String {
    format!("{prefix} {}", option.id)
}

/// Provider segment of a catalog model id (`provider/model…`).
pub(crate) fn provider_of_model_id(model_id: &str) -> &str {
    model_id.split('/').next().unwrap_or(model_id)
}

/// Unique provider labels in catalog order for `/model` Provider Tabs.
/// Empty when the catalog has fewer than two providers (no tab bar needed).
pub(crate) fn model_provider_tabs(models: &ModelState) -> Vec<String> {
    let mut tabs = Vec::new();
    for id in models.available.keys() {
        let provider = provider_of_model_id(id.0.as_ref());
        if !tabs.iter().any(|tab| tab == provider) {
            tabs.push(provider.to_string());
        }
    }
    if tabs.len() < 2 {
        tabs.clear();
    } else {
        tabs.sort();
    }
    tabs
}

/// Index of the tab that should open for the current model, or `0` when unknown.
pub(crate) fn initial_provider_tab(models: &ModelState, tabs: &[String]) -> usize {
    let Some(current) = models.current.as_ref() else {
        return 0;
    };
    let provider = provider_of_model_id(current.0.as_ref());
    tabs.iter().position(|tab| tab == provider).unwrap_or(0)
}

/// Whether an ArgPicker row belongs to `provider` (matched via catalog name → id).
pub(crate) fn arg_item_belongs_to_provider(
    models: &ModelState,
    item: &ArgItem,
    provider: &str,
) -> bool {
    let name = item.insert_text.trim_end();
    models
        .available
        .iter()
        .find(|(_, info)| info.name == name || info.name == item.match_text)
        .is_some_and(|(id, _)| provider_of_model_id(id.0.as_ref()) == provider)
}

/// Filter model-phase ArgPicker rows to one provider, optionally also by type-to-find query.
pub(crate) fn filter_model_items_for_provider(
    models: &ModelState,
    items: &[ArgItem],
    provider: &str,
    query: &str,
) -> Vec<ArgItem> {
    let q = query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| arg_item_belongs_to_provider(models, item, provider))
        .filter(|item| {
            q.is_empty()
                || item.match_text.to_lowercase().contains(&q)
                || item.display.to_lowercase().contains(&q)
                || item.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_shell::sampling::types::ReasoningEffort;
    use std::sync::Arc;

    fn model_with_reasoning(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "supportsReasoningEffort".into(),
            serde_json::Value::Bool(true),
        );
        let info = acp::ModelInfo::new(id.clone(), name.to_string())
            .meta(serde_json::Value::Object(meta).as_object().cloned());
        (id, info)
    }

    fn plain_model(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let info = acp::ModelInfo::new(id.clone(), name.to_string());
        (id, info)
    }

    static EMPTY_BUNDLE: crate::app::bundle::BundleState = crate::app::bundle::BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn dummy_exec_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &EMPTY_BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }

    #[test]
    fn split_model_effort_keeps_a_multi_word_label() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("grok-4.7", "A3S Code 4.7");
        state.available.insert(id.clone(), info);
        assert_eq!(
            split_model_effort(&state, "A3S Code 4.7 Extra High")
                .map(|(model, token)| { (model.0.to_string(), token.to_string()) }),
            Some(("grok-4.7".to_string(), "Extra High".to_string()))
        );
        assert_eq!(
            split_model_effort(&state, "A3S Code 4.7 high").map(|(_, token)| token),
            Some("high")
        );
        assert!(split_model_effort(&state, "A3S Code 4.7").is_none());
        assert_eq!(
            split_model_effort(&state, "grok-4.7 Extra High")
                .map(|(model, token)| (model.0.to_string(), token.to_string())),
            Some(("grok-4.7".to_string(), "Extra High".to_string()))
        );
    }

    #[test]
    fn empty_query_returns_one_row_per_logical_model() {
        let mut state = ModelState::default();
        let (rid, rinfo) = model_with_reasoning("reasoning-x", "Reasoning X");
        let (pid, pinfo) = plain_model("grok-4.5", "A3S Code 4.5");
        state.available.insert(rid, rinfo);
        state.available.insert(pid, pinfo);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        let items = cmd.suggest_args(&ctx, "").unwrap();
        assert_eq!(items.len(), 2, "model phase: one row per logical model");

        // A reasoning model has a trailing space in insert_text
        // The prompt widget reads it to keep the dropdown open after Enter so the effort sub-menu can render
        let reasoning = items
            .iter()
            .find(|i| i.match_text == "Reasoning X")
            .unwrap();
        assert_eq!(reasoning.insert_text, "Reasoning X ");

        // A plain model has no trailing space, so Enter commits immediately
        let plain = items
            .iter()
            .find(|i| i.match_text == "A3S Code 4.5")
            .unwrap();
        assert_eq!(plain.insert_text, "A3S Code 4.5");
    }

    #[test]
    fn trailing_space_after_reasoning_model_enters_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // The args query has a trailing space, so this is the effort phase
        // Items come out ordered low → max (cli BudgetProfile slider order) per EFFORT_LEVELS
        let items = cmd.suggest_args(&ctx, "Reasoning X ").unwrap();
        assert_eq!(items.len(), 5);
        let [a, b, c, d, e] = items.as_slice() else {
            panic!("expected 5 items: {items:?}");
        };
        assert_eq!(a.insert_text, "Reasoning X low");
        assert_eq!(b.insert_text, "Reasoning X medium");
        assert_eq!(c.insert_text, "Reasoning X high");
        assert_eq!(d.insert_text, "Reasoning X xhigh");
        assert_eq!(e.insert_text, "Reasoning X max");
        // Display is just the level so the user sees a clean column.
        assert_eq!(a.display, "low");
        // match_text carries the sort-key prefix that forces the matcher's alphabetical tiebreak to render rows in EFFORT_LEVELS order
        assert!(a.match_text.starts_with("a "));
        assert!(e.match_text.starts_with("e "));
    }

    #[test]
    fn preselected_arg_targets_default_row_only_for_fresh_effort_menu() {
        let mut state = ModelState::default();
        let id = acp::ModelId::new(Arc::from("reasoning-x"));
        let info = acp::ModelInfo::new(id.clone(), "Reasoning X").meta(
            serde_json::json!({ "supportsReasoningEffort": true, "reasoningEffort": "high" })
                .as_object()
                .cloned(),
        );
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // The preselection must name a row `suggest_args` actually builds, or the consumers fall back to row 0
        // EFFORT_LEVELS order is low → max, so `high` sits at index 2
        let high_row = cmd
            .suggest_args(&ctx, "Reasoning X ")
            .and_then(|items| items.get(2).map(|item| item.insert_text.clone()));
        assert_eq!(Some("Reasoning X high".to_owned()), high_row);
        assert_eq!(high_row, cmd.preselected_arg(&ctx, "Reasoning X "));
        assert_eq!(None, cmd.preselected_arg(&ctx, "Reasoning X h"));
        assert_eq!(None, cmd.preselected_arg(&ctx, ""));
    }

    #[test]
    fn partial_effort_query_still_in_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // Still in effort phase; the matcher upstream narrows to high and xhigh
        let items = cmd.suggest_args(&ctx, "Reasoning X h").unwrap();
        assert_eq!(items.len(), 5);
    }

    #[test]
    fn partial_model_query_stays_in_model_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        };
        // No trailing space: the user is still typing the model name
        let items = cmd.suggest_args(&ctx, "Reason").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items.first().map(|item| item.insert_text.as_str()),
            Some("Reasoning X ")
        );
    }

    #[test]
    fn run_parses_model_plus_effort_when_supported() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Reasoning X xhigh");
        match result {
            CommandResult::Action(Action::SwitchModel { model_id, effort }) => {
                assert_eq!(model_id.0.as_ref(), "reasoning-x");
                assert_eq!(effort, Some(ReasoningEffort::Xhigh));
            }
            other => panic!("expected SwitchModel with effort, got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_unoffered_effort_with_effort_error_not_unknown_model() {
        // Regression: previously `resolve_effort_token_for` returned None and the handler fell through to `Unknown model: Reasoning X none`
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Reasoning X none");
        match result {
            CommandResult::Error(msg) => {
                assert!(
                    msg.contains("unknown effort level 'none'"),
                    "expected effort error, got {msg}"
                );
                assert!(
                    msg.contains("use one of:"),
                    "expected offered levels in message, got {msg}"
                );
                assert!(
                    !msg.to_lowercase().contains("unknown model"),
                    "must not misreport as unknown model: {msg}"
                );
                let offered = msg.split_once("; ").map(|(_, r)| r).unwrap_or("");
                assert!(
                    !offered.contains("none"),
                    "must not list none as offered: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn run_prefers_full_multi_word_model_name_over_prefix_plus_effort() {
        // The catalog has both "A3S Code" (reasoning) and "A3S Code 4.5"
        // `/model A3S Code 4.5` must select the full name, not treat "4.5" as an effort on "A3S Code"
        let mut state = ModelState::default();
        let (short_id, short_info) = model_with_reasoning("grok", "A3S Code");
        let (long_id, long_info) = model_with_reasoning("grok-4.5", "A3S Code 4.5");
        state.available.insert(short_id, short_info);
        state.available.insert(long_id.clone(), long_info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "A3S Code 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, long_id);
            }
            other => panic!("expected SetDefaultModel(A3S Code 4.5), got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_effort_for_non_reasoning_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "A3S Code 4.5");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "A3S Code 4.5 high");
        // Falls through to "is the whole string a model name?", which it isn't, so we get an Unknown error
        assert!(matches!(result, CommandResult::Error(_)));
    }

    /// The bare `/model <name>` form dispatches `Action::SetDefaultModel(<ModelId>)` instead of the legacy `Action::SwitchModel { effort: None }`.
    /// The dispatcher routes it through both `Effect::SwitchModel` (session mutation) and `Effect::PersistSetting` (next-session default).
    /// The payload is the typed `acp::ModelId` (resolved at the slash boundary), not a String.
    #[test]
    fn run_bare_model_name_dispatches_set_default_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "A3S Code 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "A3S Code 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }

    /// Case-insensitive matching against the catalog: `/model a3s 4.5` resolves to the same `ModelId` as `/model A3S Code 4.5`.
    #[test]
    fn run_set_default_model_resolves_case_insensitively() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "A3S Code 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }

    #[test]
    fn provider_tabs_appear_only_with_two_or_more_providers() {
        let mut state = ModelState::default();
        let (a, ai) = plain_model("cc-switch/glm", "cc-switch · glm");
        state.available.insert(a, ai);
        assert!(model_provider_tabs(&state).is_empty());

        let (b, bi) = plain_model("grok/grok-4.7", "grok · grok-4.7");
        state.available.insert(b.clone(), bi);
        let tabs = model_provider_tabs(&state);
        assert_eq!(tabs, vec!["cc-switch".to_string(), "grok".to_string()]);

        state.current = Some(b);
        assert_eq!(initial_provider_tab(&state, &tabs), 1);

        let items = build_model_items(&state);
        let filtered = filter_model_items_for_provider(&state, &items, "grok", "");
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].match_text.contains("grok"));
    }
}
