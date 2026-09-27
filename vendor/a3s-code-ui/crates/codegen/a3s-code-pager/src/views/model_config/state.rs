//! Key handling for the provider/model modal.
//!
//! Left pane selects a provider. Right pane edits that provider's connection
//! and models, plus the default model and runtime overrides.

use std::collections::HashMap;

use a3s_acl::Block;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::document::{ConfigDraft, ModelDraft, ProviderDraft};
use crate::views::modal_window::ModalWindowState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Providers,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailField {
    ProviderId,
    BaseUrl,
    ApiKey,
    /// One catalog row for a model under the selected provider.
    ModelSummary(usize),
    ModelId(usize),
    ModelName(usize),
    ModelBaseUrl(usize),
    ModelApiKey(usize),
    ModelContext(usize),
    ModelOutput(usize),
    AddModel,
    DefaultProvider,
    DefaultModel,
    ThinkingBudget,
    TimeoutSeconds,
}

#[derive(Debug, Clone)]
struct Edit {
    buffer: String,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelConfigOutcome {
    Changed,
    Close,
    Unchanged,
}

#[derive(Debug)]
pub struct ModelConfigState {
    pub window: ModalWindowState,
    pub(crate) draft: ConfigDraft,
    pub(crate) focus: Focus,
    pub provider_index: usize,
    pub detail_index: usize,
    /// Model whose id, name, and limits are shown under its catalog row.
    expanded_model: Option<usize>,
    pub status: String,
    editing: Option<Edit>,
    pub reveal_key: bool,
}

impl ModelConfigState {
    pub(crate) fn load_default() -> Self {
        Self::from_draft(ConfigDraft::load_default())
    }

    pub(crate) fn from_draft(draft: ConfigDraft) -> Self {
        let status = if let Some(error) = &draft.load_error {
            format!("Could not read config: {error}")
        } else if draft.providers.is_empty() {
            "Press a to add a provider".to_string()
        } else {
            "Enter opens that provider's models. On the right, a adds a model.".to_string()
        };
        Self {
            window: ModalWindowState::new(),
            draft,
            focus: Focus::Providers,
            provider_index: 0,
            detail_index: 0,
            expanded_model: None,
            status,
            editing: None,
            reveal_key: false,
        }
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub(crate) fn editing_text(&self) -> Option<&str> {
        self.editing.as_ref().map(|edit| edit.buffer.as_str())
    }

    pub(crate) fn selected_provider(&self) -> Option<&ProviderDraft> {
        self.draft.providers.get(self.provider_index)
    }

    pub(crate) fn detail_fields(&self) -> Vec<DetailField> {
        let mut fields = Vec::new();
        if let Some(provider) = self.selected_provider() {
            fields.push(DetailField::ProviderId);
            fields.push(DetailField::BaseUrl);
            fields.push(DetailField::ApiKey);
            for index in 0..provider.models.len() {
                fields.push(DetailField::ModelSummary(index));
                if self.expanded_model == Some(index) {
                    fields.push(DetailField::ModelId(index));
                    fields.push(DetailField::ModelName(index));
                    fields.push(DetailField::ModelBaseUrl(index));
                    fields.push(DetailField::ModelApiKey(index));
                    fields.push(DetailField::ModelContext(index));
                    fields.push(DetailField::ModelOutput(index));
                }
            }
            fields.push(DetailField::AddModel);
        }
        fields.push(DetailField::DefaultProvider);
        fields.push(DetailField::DefaultModel);
        fields.push(DetailField::ThinkingBudget);
        fields.push(DetailField::TimeoutSeconds);
        fields
    }

    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> ModelConfigOutcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
                self.reveal_key = !self.reveal_key;
                return ModelConfigOutcome::Changed;
            }
            return ModelConfigOutcome::Unchanged;
        }
        if self.editing.is_some() {
            return self.handle_edit_key(key);
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Tab | KeyCode::BackTab => {
                match self.focus {
                    Focus::Providers => self.open_models(),
                    Focus::Detail => {
                        self.focus = Focus::Providers;
                        self.editing = None;
                    }
                }
                ModelConfigOutcome::Changed
            }
            KeyCode::Left => {
                self.focus = Focus::Providers;
                ModelConfigOutcome::Changed
            }
            KeyCode::Right => {
                self.open_models();
                ModelConfigOutcome::Changed
            }
            KeyCode::Enter => self.activate(),
            KeyCode::Char('a') => self.add_current(),
            KeyCode::Char('x') => self.remove_current(),
            KeyCode::Char(' ') => self.toggle_tools(),
            KeyCode::Char('s') => self.save(),
            KeyCode::Char('u') => self.reload(),
            KeyCode::Char('[') => self.cycle_default(-1),
            KeyCode::Char(']') => self.cycle_default(1),
            _ => ModelConfigOutcome::Unchanged,
        }
    }

    pub(crate) fn paste(&mut self, text: &str) -> bool {
        let Some(edit) = self.editing.as_mut() else {
            return false;
        };
        let clean: String = text
            .chars()
            .filter(|ch| *ch != '\n' && *ch != '\r')
            .collect();
        if clean.is_empty() {
            return false;
        }
        let byte = byte_index(&edit.buffer, edit.cursor);
        edit.buffer.insert_str(byte, &clean);
        edit.cursor += clean.chars().count();
        true
    }

    fn handle_edit_key(&mut self, key: &KeyEvent) -> ModelConfigOutcome {
        match key.code {
            KeyCode::Esc => {
                self.editing = None;
                ModelConfigOutcome::Changed
            }
            KeyCode::Enter => {
                self.commit_edit();
                ModelConfigOutcome::Changed
            }
            KeyCode::Left => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.cursor = edit.cursor.saturating_sub(1);
                }
                ModelConfigOutcome::Changed
            }
            KeyCode::Right => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.cursor < edit.buffer.chars().count() {
                        edit.cursor += 1;
                    }
                }
                ModelConfigOutcome::Changed
            }
            KeyCode::Backspace => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.cursor > 0 {
                        let byte = byte_index(&edit.buffer, edit.cursor);
                        let prev = byte_index(&edit.buffer, edit.cursor - 1);
                        edit.buffer.replace_range(prev..byte, "");
                        edit.cursor -= 1;
                    }
                }
                ModelConfigOutcome::Changed
            }
            KeyCode::Char(ch) => {
                if let Some(edit) = self.editing.as_mut() {
                    let byte = byte_index(&edit.buffer, edit.cursor);
                    edit.buffer.insert(byte, ch);
                    edit.cursor += 1;
                }
                ModelConfigOutcome::Changed
            }
            _ => ModelConfigOutcome::Unchanged,
        }
    }

    fn move_selection(&mut self, delta: isize) -> ModelConfigOutcome {
        match self.focus {
            Focus::Providers => {
                let len = self.draft.providers.len() + 1;
                self.provider_index = step(self.provider_index, len, delta);
                self.detail_index = 0;
                self.expanded_model = None;
            }
            Focus::Detail => {
                let fields = self.detail_fields();
                if fields.is_empty() {
                    return ModelConfigOutcome::Changed;
                }
                let next = fields[step(self.detail_index, fields.len(), delta)];
                self.focus_field(next);
            }
        }
        ModelConfigOutcome::Changed
    }

    fn activate(&mut self) -> ModelConfigOutcome {
        if self.focus == Focus::Providers {
            if self.provider_index == self.draft.providers.len() {
                self.add_provider();
            } else {
                self.open_models();
            }
            return ModelConfigOutcome::Changed;
        }
        let Some(field) = self.current_field() else {
            return ModelConfigOutcome::Unchanged;
        };
        if matches!(field, DetailField::AddModel) {
            self.add_model();
            return ModelConfigOutcome::Changed;
        }
        if let DetailField::ModelSummary(index) = field {
            self.focus_field(DetailField::ModelId(index));
            return self.activate();
        }
        let text = self.field_text(field).unwrap_or_default();
        self.editing = Some(Edit {
            cursor: text.chars().count(),
            buffer: text,
        });
        ModelConfigOutcome::Changed
    }

    fn add_current(&mut self) -> ModelConfigOutcome {
        if self.focus == Focus::Providers || self.selected_provider().is_none() {
            self.add_provider();
        } else {
            self.add_model();
        }
        ModelConfigOutcome::Changed
    }

    fn add_provider(&mut self) {
        let id = unique_id("provider", &self.provider_ids());
        self.draft.providers.push(empty_provider(id));
        self.provider_index = self.draft.providers.len() - 1;
        self.expanded_model = None;
        self.focus_field(DetailField::AddModel);
        self.focus = Focus::Detail;
        self.status = "Provider added. Press a to add its first model.".to_string();
    }

    fn add_model(&mut self) {
        let (provider_id, model_index) = {
            let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
                self.status = "Add a provider first".to_string();
                return;
            };
            let ids = provider
                .models
                .iter()
                .map(|model| model.id.clone())
                .collect::<Vec<_>>();
            let id = unique_id("model", &ids);
            provider.models.push(empty_model(id));
            provider.dirty = true;
            let index = provider.models.len() - 1;
            (provider.id.clone(), index)
        };
        self.focus = Focus::Detail;
        self.focus_field(DetailField::ModelId(model_index));
        let text = self
            .field_text(DetailField::ModelId(model_index))
            .unwrap_or_default();
        self.editing = Some(Edit {
            cursor: text.chars().count(),
            buffer: text,
        });
        self.status = format!(
            "Added a model under {provider_id}. Type its id, Enter, then Down for the name. a adds another."
        );
    }

    fn remove_current(&mut self) -> ModelConfigOutcome {
        if self.focus == Focus::Providers {
            if self.provider_index >= self.draft.providers.len() {
                return ModelConfigOutcome::Unchanged;
            }
            let removed = self.draft.providers.remove(self.provider_index);
            self.forget_default_prefix(&format!("{}/", removed.id));
            if self.provider_index > self.draft.providers.len() {
                self.provider_index = self.draft.providers.len();
            }
            self.status = format!("Removed provider {}", removed.id);
            return ModelConfigOutcome::Changed;
        }
        let Some(field) = self.current_field() else {
            return ModelConfigOutcome::Unchanged;
        };
        let model_index = match field {
            DetailField::ModelSummary(index)
            | DetailField::ModelId(index)
            | DetailField::ModelName(index)
            | DetailField::ModelBaseUrl(index)
            | DetailField::ModelApiKey(index)
            | DetailField::ModelContext(index)
            | DetailField::ModelOutput(index) => index,
            _ => return ModelConfigOutcome::Unchanged,
        };
        let removed = {
            let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
                return ModelConfigOutcome::Unchanged;
            };
            if model_index >= provider.models.len() {
                return ModelConfigOutcome::Unchanged;
            }
            let removed = provider.models.remove(model_index);
            provider.dirty = true;
            (provider.id.clone(), removed.id)
        };
        self.forget_default(&format!("{}/{}", removed.0, removed.1));
        let remaining = self
            .selected_provider()
            .map(|provider| provider.models.len())
            .unwrap_or(0);
        if remaining == 0 {
            self.expanded_model = None;
            self.focus_field(DetailField::AddModel);
        } else {
            let next = model_index.min(remaining - 1);
            self.focus_field(DetailField::ModelSummary(next));
        }
        self.status = format!(
            "Removed model {}. a adds another under this provider.",
            removed.1
        );
        ModelConfigOutcome::Changed
    }

    fn toggle_tools(&mut self) -> ModelConfigOutcome {
        let Some(
            DetailField::ModelSummary(index)
            | DetailField::ModelId(index)
            | DetailField::ModelName(index)
            | DetailField::ModelBaseUrl(index)
            | DetailField::ModelApiKey(index)
            | DetailField::ModelContext(index)
            | DetailField::ModelOutput(index),
        ) = self.current_field()
        else {
            return ModelConfigOutcome::Unchanged;
        };
        let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
            return ModelConfigOutcome::Unchanged;
        };
        let Some(model) = provider.models.get_mut(index) else {
            return ModelConfigOutcome::Unchanged;
        };
        model.tool_call = !model.tool_call;
        provider.dirty = true;
        self.status = if model.tool_call {
            "Tool calling on".to_string()
        } else {
            "Tool calling off".to_string()
        };
        ModelConfigOutcome::Changed
    }

    fn save(&mut self) -> ModelConfigOutcome {
        let path = self.draft.path.clone();
        match self.draft.save() {
            Ok(()) => {
                self.editing = None;
                self.status = format!("Saved {}. Restart a3s code to use it", path.display());
            }
            Err(error) => self.status = error,
        }
        self.clamp();
        ModelConfigOutcome::Changed
    }

    fn reload(&mut self) -> ModelConfigOutcome {
        let path = self.draft.path.clone();
        self.draft = ConfigDraft::load(path);
        self.editing = None;
        self.clamp();
        self.status = "Reloaded from disk".to_string();
        ModelConfigOutcome::Changed
    }

    fn cycle_default(&mut self, delta: isize) -> ModelConfigOutcome {
        if matches!(self.current_field(), Some(DetailField::DefaultProvider)) {
            self.cycle_default_provider(delta)
        } else {
            self.cycle_default_model(delta)
        }
    }

    fn cycle_default_provider(&mut self, delta: isize) -> ModelConfigOutcome {
        if self.draft.providers.is_empty() {
            self.status = "Add a provider before choosing a default".to_string();
            return ModelConfigOutcome::Changed;
        }
        let ids = self
            .draft
            .providers
            .iter()
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        let (provider_id, model_id) = split_default(&self.draft.default_model);
        let current = ids.iter().position(|id| id == &provider_id).unwrap_or(0);
        let next = ids[step(current, ids.len(), delta)].clone();
        let model = self
            .draft
            .providers
            .iter()
            .find(|provider| provider.id == next)
            .and_then(|provider| {
                if provider.models.iter().any(|model| model.id == model_id) {
                    Some(model_id)
                } else {
                    provider.models.first().map(|model| model.id.clone())
                }
            })
            .unwrap_or_default();
        self.set_default(&next, &model);
        ModelConfigOutcome::Changed
    }

    fn cycle_default_model(&mut self, delta: isize) -> ModelConfigOutcome {
        let (provider_id, model_id) = split_default(&self.draft.default_model);
        let provider = if provider_id.is_empty() {
            self.selected_provider()
                .or_else(|| self.draft.providers.first())
        } else {
            self.draft
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .or_else(|| self.draft.providers.first())
        };
        let Some(provider) = provider else {
            self.status = "Add a provider before choosing a default".to_string();
            return ModelConfigOutcome::Changed;
        };
        if provider.models.is_empty() {
            self.status = format!(
                "Add a model under {} before choosing a default",
                provider.id
            );
            return ModelConfigOutcome::Changed;
        }
        let current = provider
            .models
            .iter()
            .position(|model| model.id == model_id)
            .unwrap_or(0);
        let next = step(current, provider.models.len(), delta);
        let model = provider.models[next].id.clone();
        let provider_id = provider.id.clone();
        self.set_default(&provider_id, &model);
        ModelConfigOutcome::Changed
    }

    fn set_default(&mut self, provider_id: &str, model_id: &str) {
        self.draft.default_model = if model_id.is_empty() {
            provider_id.to_string()
        } else {
            format!("{provider_id}/{model_id}")
        };
        self.status = format!(
            "Default provider {provider_id}, model {}. Save to write config.acl",
            if model_id.is_empty() {
                "not set"
            } else {
                model_id
            }
        );
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let Some(field) = self.current_field() else {
            return;
        };
        let text = edit.buffer;
        match field {
            DetailField::ProviderId => self.commit_provider_id(text),
            DetailField::BaseUrl => self.commit_provider_text(text, false),
            DetailField::ApiKey => self.commit_provider_text(text, true),
            DetailField::ModelSummary(_) | DetailField::AddModel => {}
            DetailField::ModelId(index) => self.commit_model_id(index, text),
            DetailField::ModelName(index) => self.commit_model_name(index, text),
            DetailField::ModelBaseUrl(index) => self.commit_model_connection(index, text, false),
            DetailField::ModelApiKey(index) => self.commit_model_connection(index, text, true),
            DetailField::ModelContext(index) => self.commit_model_limit(index, text, true),
            DetailField::ModelOutput(index) => self.commit_model_limit(index, text, false),
            DetailField::DefaultProvider => self.commit_default_provider(text),
            DetailField::DefaultModel => self.commit_default_model(text),
            DetailField::ThinkingBudget => {
                self.draft.thinking_budget = text.trim().to_string();
                self.status = "Reasoning budget updated. Save to write config.acl".to_string();
            }
            DetailField::TimeoutSeconds => {
                self.draft.timeout_seconds = text.trim().to_string();
                self.status = "Timeout updated. Save to write config.acl".to_string();
            }
        }
    }

    fn commit_provider_id(&mut self, text: String) {
        let id = text.trim().to_string();
        if id.is_empty() {
            self.status = "Provider id cannot be empty".to_string();
            return;
        }
        if self
            .draft
            .providers
            .iter()
            .enumerate()
            .any(|(index, provider)| index != self.provider_index && provider.id == id)
        {
            self.status = format!("Provider id `{id}` is already used");
            return;
        }
        let previous = {
            let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
                return;
            };
            if provider.id == id {
                return;
            }
            let previous = provider.id.clone();
            provider.id = id.clone();
            provider.dirty = true;
            previous
        };
        let prefix_from = format!("{previous}/");
        if let Some(rest) = self.draft.default_model.strip_prefix(&prefix_from) {
            self.draft.default_model = format!("{id}/{rest}");
        }
        self.status = format!("Provider id is {id}. Save to write config.acl");
    }

    fn commit_provider_text(&mut self, text: String, api_key: bool) {
        let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
            return;
        };
        if api_key {
            provider.api_key = text;
        } else {
            provider.base_url = text.trim().to_string();
        }
        provider.dirty = true;
        self.status = "Connection updated. Save to write config.acl".to_string();
    }

    fn commit_model_id(&mut self, index: usize, text: String) {
        let id = text.trim().to_string();
        if id.is_empty() {
            self.status = "Model id cannot be empty".to_string();
            return;
        }
        let renamed = {
            let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
                return;
            };
            if provider
                .models
                .iter()
                .enumerate()
                .any(|(other, model)| other != index && model.id == id)
            {
                None
            } else if provider
                .models
                .get(index)
                .is_none_or(|model| model.id == id)
            {
                Some(None)
            } else {
                let previous = provider.models[index].id.clone();
                provider.models[index].id = id.clone();
                provider.dirty = true;
                Some(Some((provider.id.clone(), previous)))
            }
        };
        let Some(renamed) = renamed else {
            self.status = format!("Model id `{id}` must be unique in this provider");
            return;
        };
        let Some((provider_id, previous)) = renamed else {
            return;
        };
        let from = format!("{provider_id}/{previous}");
        if self.draft.default_model == from {
            self.draft.default_model = format!("{provider_id}/{id}");
        }
        self.status = format!("Model id is {id}. Save to write config.acl");
    }

    fn commit_model_name(&mut self, index: usize, text: String) {
        let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
            return;
        };
        let Some(model) = provider.models.get_mut(index) else {
            return;
        };
        model.name = text.trim().to_string();
        provider.dirty = true;
        self.status = "Model name updated. Save to write config.acl".to_string();
    }

    fn commit_model_connection(&mut self, index: usize, text: String, api_key: bool) {
        let cleared = {
            let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
                return;
            };
            let Some(model) = provider.models.get_mut(index) else {
                return;
            };
            if api_key {
                model.api_key = text;
            } else {
                model.base_url = text.trim().to_string();
            }
            provider.dirty = true;
            if api_key {
                model.api_key.is_empty()
            } else {
                model.base_url.is_empty()
            }
        };
        self.status = if cleared {
            "Blank. This model inherits the provider value. Save to write config.acl".to_string()
        } else {
            "This model uses its own connection value. Save to write config.acl".to_string()
        };
    }

    fn commit_default_provider(&mut self, text: String) {
        let provider_id = text.trim().to_string();
        if provider_id.is_empty() {
            self.draft.default_model.clear();
            self.status = "Default cleared. Save to write config.acl".to_string();
            return;
        }
        let (_, model_id) = split_default(&self.draft.default_model);
        let model = self
            .draft
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .and_then(|provider| {
                if provider.models.iter().any(|model| model.id == model_id) {
                    Some(model_id)
                } else {
                    provider.models.first().map(|model| model.id.clone())
                }
            })
            .unwrap_or_default();
        self.set_default(&provider_id, &model);
    }

    fn commit_default_model(&mut self, text: String) {
        let model_id = text.trim().to_string();
        let (provider_id, _) = split_default(&self.draft.default_model);
        let provider_id = if provider_id.is_empty() {
            self.selected_provider()
                .or_else(|| self.draft.providers.first())
                .map(|provider| provider.id.clone())
                .unwrap_or_default()
        } else {
            provider_id
        };
        if provider_id.is_empty() {
            self.status = "Choose a default provider first".to_string();
            return;
        }
        self.set_default(&provider_id, &model_id);
    }

    fn commit_model_limit(&mut self, index: usize, text: String, context: bool) {
        let parsed = if text.trim().is_empty() {
            0
        } else {
            match text.trim().parse::<u32>() {
                Ok(value) => value,
                Err(_) => {
                    self.status = "Token limit must be a whole number".to_string();
                    return;
                }
            }
        };
        let Some(provider) = self.draft.providers.get_mut(self.provider_index) else {
            return;
        };
        let Some(model) = provider.models.get_mut(index) else {
            return;
        };
        if context {
            model.context = parsed;
        } else {
            model.output = parsed;
        }
        provider.dirty = true;
        self.status = "Token limit updated. Save to write config.acl".to_string();
    }

    fn current_field(&self) -> Option<DetailField> {
        self.detail_fields().get(self.detail_index).copied()
    }

    fn open_models(&mut self) {
        self.focus = Focus::Detail;
        self.editing = None;
        if self
            .selected_provider()
            .is_some_and(|provider| !provider.models.is_empty())
        {
            self.focus_field(DetailField::ModelSummary(0));
            self.status = "Models under this provider. a adds one. Enter edits it. A blank API URL or key inherits the provider.".to_string();
        } else if self.selected_provider().is_some() {
            self.focus_field(DetailField::AddModel);
            self.status = "This provider has no models yet. Press a to add one.".to_string();
        } else {
            self.status = "Add a provider first".to_string();
        }
    }

    fn focus_field(&mut self, field: DetailField) {
        if let Some(index) = model_index(field) {
            self.expanded_model = Some(index);
        }
        let fields = self.detail_fields();
        self.detail_index = fields.iter().position(|item| *item == field).unwrap_or(0);
    }

    pub(crate) fn field_text(&self, field: DetailField) -> Option<String> {
        let provider = self.selected_provider();
        Some(match field {
            DetailField::ProviderId => provider?.id.clone(),
            DetailField::BaseUrl => provider?.base_url.clone(),
            DetailField::ApiKey => provider?.api_key.clone(),
            DetailField::ModelSummary(index) | DetailField::ModelId(index) => {
                provider?.models.get(index)?.id.clone()
            }
            DetailField::ModelName(index) => provider?.models.get(index)?.name.clone(),
            DetailField::ModelBaseUrl(index) => provider?.models.get(index)?.base_url.clone(),
            DetailField::ModelApiKey(index) => provider?.models.get(index)?.api_key.clone(),
            DetailField::ModelContext(index) => provider?.models.get(index)?.context.to_string(),
            DetailField::ModelOutput(index) => provider?.models.get(index)?.output.to_string(),
            DetailField::DefaultProvider => split_default(&self.draft.default_model).0,
            DetailField::DefaultModel => split_default(&self.draft.default_model).1,
            DetailField::ThinkingBudget => self.draft.thinking_budget.clone(),
            DetailField::TimeoutSeconds => self.draft.timeout_seconds.clone(),
            DetailField::AddModel => return None,
        })
    }

    fn catalog(&self) -> Vec<String> {
        self.draft
            .providers
            .iter()
            .flat_map(|provider| {
                provider
                    .models
                    .iter()
                    .map(|model| format!("{}/{}", provider.id, model.id))
            })
            .collect()
    }

    fn provider_ids(&self) -> Vec<String> {
        self.draft
            .providers
            .iter()
            .map(|provider| provider.id.clone())
            .collect()
    }

    fn forget_default(&mut self, id: &str) {
        if self.draft.default_model == id {
            self.draft.default_model = self.catalog().into_iter().next().unwrap_or_default();
        }
    }

    fn forget_default_prefix(&mut self, prefix: &str) {
        if self.draft.default_model.starts_with(prefix) {
            self.draft.default_model = self.catalog().into_iter().next().unwrap_or_default();
        }
    }

    fn clamp(&mut self) {
        let max_provider = self.draft.providers.len();
        if self.provider_index > max_provider {
            self.provider_index = max_provider;
        }
        let len = self.detail_fields().len();
        if len == 0 {
            self.detail_index = 0;
        } else if self.detail_index >= len {
            self.detail_index = len - 1;
        }
        let model_count = self
            .selected_provider()
            .map(|provider| provider.models.len())
            .unwrap_or(0);
        if model_count == 0 {
            self.expanded_model = None;
        } else if self
            .expanded_model
            .is_some_and(|index| index >= model_count)
        {
            self.expanded_model = Some(model_count - 1);
        }
    }
}

fn model_index(field: DetailField) -> Option<usize> {
    match field {
        DetailField::ModelSummary(index)
        | DetailField::ModelId(index)
        | DetailField::ModelName(index)
        | DetailField::ModelBaseUrl(index)
        | DetailField::ModelApiKey(index)
        | DetailField::ModelContext(index)
        | DetailField::ModelOutput(index) => Some(index),
        _ => None,
    }
}

fn split_default(value: &str) -> (String, String) {
    match value.split_once('/') {
        Some((provider, model)) => (provider.to_string(), model.to_string()),
        None => (value.to_string(), String::new()),
    }
}

fn step(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = current as isize + delta;
    if next < 0 {
        len - 1
    } else if next as usize >= len {
        0
    } else {
        next as usize
    }
}

fn byte_index(text: &str, cursor: usize) -> usize {
    text.char_indices()
        .nth(cursor)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

fn unique_id(prefix: &str, existing: &[String]) -> String {
    let mut n = existing.len() + 1;
    loop {
        let id = format!("{prefix}-{n}");
        if !existing.iter().any(|item| item == &id) {
            return id;
        }
        n += 1;
    }
}

fn empty_provider(id: String) -> ProviderDraft {
    ProviderDraft {
        id: id.clone(),
        original_label: None,
        api_key: String::new(),
        api_key_original: None,
        base_url: String::new(),
        base_url_original: None,
        models: Vec::new(),
        template: Block {
            name: "providers".to_string(),
            labels: vec![id],
            blocks: Vec::new(),
            attributes: HashMap::new(),
        },
        dirty: true,
    }
}

fn empty_model(id: String) -> ModelDraft {
    ModelDraft {
        id: id.clone(),
        name: "New model".to_string(),
        context: 0,
        output: 0,
        tool_call: true,
        api_key: String::new(),
        api_key_original: None,
        base_url: String::new(),
        base_url_original: None,
        template: Block {
            name: "models".to_string(),
            labels: vec![id],
            blocks: Vec::new(),
            attributes: HashMap::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn add_provider_and_model_then_reject_a_duplicate_id() {
        let mut state = ModelConfigState::from_draft(ConfigDraft::from_source(
            std::path::PathBuf::from("config.acl"),
            String::new(),
        ));
        assert_eq!(
            state.handle_key(&key(KeyCode::Char('a'))),
            ModelConfigOutcome::Changed
        );
        assert_eq!(state.draft.providers.len(), 1);
        assert_eq!(
            state.handle_key(&key(KeyCode::Char('a'))),
            ModelConfigOutcome::Changed
        );
        assert_eq!(state.draft.providers[0].models.len(), 1);
        state.handle_key(&key(KeyCode::Enter));
        state.handle_key(&key(KeyCode::Char('a')));
        assert_eq!(state.draft.providers[0].models.len(), 2);
        assert!(state.is_editing(), "adding a model starts the id editor");
        state.editing.as_mut().expect("edit").buffer = "model-1".to_string();
        state.handle_key(&key(KeyCode::Enter));
        assert!(
            state.status.contains("unique"),
            "status was {}",
            state.status
        );
        assert_eq!(state.draft.providers[0].models[1].id, "model-2");
    }

    #[test]
    fn default_provider_and_model_are_chosen_separately() {
        let source = r#"
default_model = "alpha/one"
providers "alpha" {
  apiKey = "a"
  baseUrl = "https://alpha.example"
  models "one" { name = "One" }
  models "two" { name = "Two" }
}
providers "beta" {
  apiKey = "b"
  baseUrl = "https://beta.example"
  models "only" { name = "Only" }
}
"#;
        let mut state = ModelConfigState::from_draft(ConfigDraft::from_source(
            std::path::PathBuf::from("config.acl"),
            source.to_string(),
        ));
        assert!(state.draft.providers[0].models[0].api_key.is_empty());
        assert!(state.draft.providers[0].models[0].base_url.is_empty());
        state.focus = Focus::Detail;
        state.focus_field(DetailField::DefaultProvider);
        state.handle_key(&key(KeyCode::Char(']')));
        assert_eq!(state.draft.default_model, "beta/only");
        state.focus_field(DetailField::DefaultProvider);
        state.handle_key(&key(KeyCode::Char('[')));
        assert_eq!(state.draft.default_model, "alpha/one");
        state.focus_field(DetailField::DefaultModel);
        state.handle_key(&key(KeyCode::Char(']')));
        assert_eq!(state.draft.default_model, "alpha/two");
    }

    #[test]
    fn models_are_a_list_under_the_provider() {
        let mut state = ModelConfigState::from_draft(ConfigDraft::from_source(
            std::path::PathBuf::from("config.acl"),
            String::new(),
        ));
        state.handle_key(&key(KeyCode::Char('a')));
        state.handle_key(&key(KeyCode::Char('a')));
        state.handle_key(&key(KeyCode::Enter));
        state.handle_key(&key(KeyCode::Char('a')));
        state.handle_key(&key(KeyCode::Esc));
        let fields = state.detail_fields();
        let summaries = fields
            .iter()
            .filter(|field| matches!(field, DetailField::ModelSummary(_)))
            .count();
        assert_eq!(summaries, 2);
        assert!(fields.contains(&DetailField::AddModel));
        assert_eq!(
            fields
                .iter()
                .filter(|field| matches!(field, DetailField::ModelId(_)))
                .count(),
            1,
            "only the highlighted model opens its editor"
        );
    }
}
