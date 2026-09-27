//! Read and write provider/model sections of an A3S ACL config.
//!
//! Untouched top-level entries stay byte-for-byte. Edited providers are rendered
//! with `a3s-acl` from the parsed block so unknown attributes survive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use a3s_acl::{Block, Document, Value, generate_acl, parse_acl};

#[derive(Debug, Clone)]
pub(crate) struct OriginalValue {
    pub display: String,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelDraft {
    pub id: String,
    pub name: String,
    pub context: u32,
    pub output: u32,
    pub tool_call: bool,
    /// Empty inherits the provider API key.
    pub api_key: String,
    pub api_key_original: Option<OriginalValue>,
    /// Empty inherits the provider base URL.
    pub base_url: String,
    pub base_url_original: Option<OriginalValue>,
    pub template: Block,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderDraft {
    pub id: String,
    pub original_label: Option<String>,
    pub api_key: String,
    pub api_key_original: Option<OriginalValue>,
    pub base_url: String,
    pub base_url_original: Option<OriginalValue>,
    pub models: Vec<ModelDraft>,
    pub template: Block,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigDraft {
    pub path: PathBuf,
    pub source: String,
    pub providers: Vec<ProviderDraft>,
    pub default_model: String,
    pub default_model_saved: String,
    pub thinking_budget: String,
    pub thinking_budget_saved: String,
    pub timeout_seconds: String,
    pub timeout_seconds_saved: String,
    pub load_error: Option<String>,
}

impl ConfigDraft {
    pub(crate) fn load_default() -> Self {
        let path = default_config_path();
        Self::load(path)
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(source) => Self::from_source(path, source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut draft = Self::from_source(path, String::new());
                draft.load_error = None;
                draft
            }
            Err(error) => Self::unreadable(path, error.to_string()),
        }
    }

    pub(crate) fn from_source(path: PathBuf, source: String) -> Self {
        match parse_draft(&path, &source) {
            Ok(draft) => draft,
            Err(error) => Self::unreadable(path, error),
        }
    }

    pub(crate) fn save(&mut self) -> Result<(), String> {
        validate(self)?;
        let text = rewrite(&self.source, self)?;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
        }
        std::fs::write(&self.path, &text).map_err(|error| error.to_string())?;
        *self = Self::from_source(self.path.clone(), text);
        Ok(())
    }

    fn unreadable(path: PathBuf, error: String) -> Self {
        Self {
            path,
            source: String::new(),
            providers: Vec::new(),
            default_model: String::new(),
            default_model_saved: String::new(),
            thinking_budget: String::new(),
            thinking_budget_saved: String::new(),
            timeout_seconds: String::new(),
            timeout_seconds_saved: String::new(),
            load_error: Some(error),
        }
    }
}

pub(crate) fn default_config_path() -> PathBuf {
    config_path_from(
        std::env::var_os("A3S_CONFIG").as_deref(),
        std::env::var_os("A3S_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        &|path| path.is_file(),
    )
}

pub(crate) fn config_path_from(
    explicit: Option<&std::ffi::OsStr>,
    a3s_home: Option<&std::ffi::OsStr>,
    user_home: Option<&std::ffi::OsStr>,
    is_file: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(path) = explicit {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Some(home) = a3s_home.map(PathBuf::from) {
        let direct = home.join("config.acl");
        let nested = home.join(".a3s").join("config.acl");
        if is_file(&direct) {
            return direct;
        }
        if is_file(&nested) {
            return nested;
        }
        if home.file_name().and_then(|name| name.to_str()) == Some(".a3s") {
            return direct;
        }
    }
    if let Some(home) = user_home {
        if !home.is_empty() {
            return PathBuf::from(home).join(".a3s").join("config.acl");
        }
    }
    PathBuf::from(".a3s").join("config.acl")
}

fn parse_draft(path: &Path, source: &str) -> Result<ConfigDraft, String> {
    if source.trim().is_empty() {
        return Ok(ConfigDraft {
            path: path.to_path_buf(),
            source: source.to_string(),
            providers: Vec::new(),
            default_model: String::new(),
            default_model_saved: String::new(),
            thinking_budget: String::new(),
            thinking_budget_saved: String::new(),
            timeout_seconds: String::new(),
            timeout_seconds_saved: String::new(),
            load_error: None,
        });
    }
    let document = parse_acl(source).map_err(|error| error.to_string())?;
    let mut providers = Vec::new();
    let mut default_model = String::new();
    let mut thinking_budget = String::new();
    let mut timeout_seconds = String::new();
    for block in &document.blocks {
        match block.name.as_str() {
            "providers" => providers.push(provider_from_block(block)),
            "default_model" | "defaultModel" => {
                if let Some(value) = string_attr(block, &["default_model", "defaultModel"]) {
                    default_model = value;
                }
            }
            "thinking_budget" | "thinkingBudget" => {
                if let Some(value) = number_text(block, &["thinking_budget", "thinkingBudget"]) {
                    thinking_budget = value;
                }
            }
            "llm_api_timeout_ms"
            | "llmApiTimeoutMs"
            | "api_timeout_ms"
            | "model_api_timeout_ms" => {
                if let Some(ms) = number_value(
                    block,
                    &[
                        "llm_api_timeout_ms",
                        "llmApiTimeoutMs",
                        "api_timeout_ms",
                        "model_api_timeout_ms",
                    ],
                ) {
                    timeout_seconds = milliseconds_to_seconds(ms);
                }
            }
            _ => {}
        }
    }
    Ok(ConfigDraft {
        path: path.to_path_buf(),
        source: source.to_string(),
        providers,
        default_model: default_model.clone(),
        default_model_saved: default_model,
        thinking_budget: thinking_budget.clone(),
        thinking_budget_saved: thinking_budget,
        timeout_seconds: timeout_seconds.clone(),
        timeout_seconds_saved: timeout_seconds,
        load_error: None,
    })
}

fn provider_from_block(block: &Block) -> ProviderDraft {
    let id = block.labels.first().cloned().unwrap_or_default();
    let api_key_original = original_attr(block, &["apiKey", "api_key"]);
    let base_url_original = original_attr(block, &["baseUrl", "base_url"]);
    let models = block
        .blocks
        .iter()
        .filter(|child| child.name == "models" || child.name == "model")
        .map(model_from_block)
        .collect();
    ProviderDraft {
        id,
        original_label: block.labels.first().cloned(),
        api_key: api_key_original
            .as_ref()
            .map(|value| value.display.clone())
            .unwrap_or_default(),
        api_key_original,
        base_url: base_url_original
            .as_ref()
            .map(|value| value.display.clone())
            .unwrap_or_default(),
        base_url_original,
        models,
        template: block.clone(),
        dirty: false,
    }
}

fn model_from_block(block: &Block) -> ModelDraft {
    let id = block.labels.first().cloned().unwrap_or_default();
    let name = string_attr(block, &["name"]).unwrap_or_else(|| id.clone());
    let (context, output) = limit_of(block);
    let tool_call = bool_attr(block, &["toolCall", "tool_call"]).unwrap_or(true);
    let api_key_original = original_attr(block, &["apiKey", "api_key"]);
    let base_url_original = original_attr(block, &["baseUrl", "base_url"]);
    ModelDraft {
        id,
        name,
        context,
        output,
        tool_call,
        api_key: api_key_original
            .as_ref()
            .map(|value| value.display.clone())
            .unwrap_or_default(),
        api_key_original,
        base_url: base_url_original
            .as_ref()
            .map(|value| value.display.clone())
            .unwrap_or_default(),
        base_url_original,
        template: block.clone(),
    }
}

fn validate(draft: &ConfigDraft) -> Result<(), String> {
    let mut seen_providers = HashMap::<String, ()>::new();
    for provider in &draft.providers {
        let id = provider.id.trim();
        if id.is_empty() {
            return Err("Provider id cannot be empty".to_string());
        }
        if seen_providers.insert(id.to_string(), ()).is_some() {
            return Err(format!("Provider id `{id}` is already used"));
        }
        let mut seen_models = HashMap::<String, ()>::new();
        for model in &provider.models {
            let model_id = model.id.trim();
            if model_id.is_empty() {
                return Err(format!("Model id in `{id}` cannot be empty"));
            }
            if seen_models.insert(model_id.to_string(), ()).is_some() {
                return Err(format!("Model id `{model_id}` must be unique in `{id}`"));
            }
        }
    }
    if !draft.default_model.trim().is_empty() {
        let Some((provider_id, model_id)) = draft.default_model.split_once('/') else {
            return Err("Default must be provider/model".to_string());
        };
        if provider_id.is_empty() || model_id.is_empty() {
            return Err("Choose a default provider and a default model".to_string());
        }
        let Some(provider) = draft
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
        else {
            return Err(format!(
                "Default provider `{provider_id}` is not in the list"
            ));
        };
        if !provider.models.iter().any(|model| model.id == model_id) {
            return Err(format!(
                "Default model `{model_id}` is not under `{provider_id}`"
            ));
        }
    }
    if !draft.thinking_budget.trim().is_empty() {
        let budget = draft
            .thinking_budget
            .trim()
            .parse::<u64>()
            .map_err(|_| "Reasoning budget must be a whole number of tokens".to_string())?;
        if budget == 0 {
            return Err("Reasoning budget must be at least 1".to_string());
        }
    }
    if !draft.timeout_seconds.trim().is_empty() {
        parse_timeout_ms(draft.timeout_seconds.trim())?;
    }
    Ok(())
}

fn rewrite(source: &str, draft: &ConfigDraft) -> Result<String, String> {
    let entries = scan_top_level_entries(source);
    let mut used = vec![false; draft.providers.len()];
    let mut edits = Vec::new();
    let mut replaced_default = false;
    let mut replaced_thinking = false;
    let mut replaced_timeout = false;
    let default_dirty = draft.default_model != draft.default_model_saved;
    let thinking_dirty = draft.thinking_budget != draft.thinking_budget_saved;
    let timeout_dirty = draft.timeout_seconds != draft.timeout_seconds_saved;

    for entry in &entries {
        if is_default_model(&entry.name) {
            if default_dirty {
                let replacement = if replaced_default {
                    String::new()
                } else {
                    replaced_default = true;
                    render_default(&draft.default_model)
                };
                edits.push((entry.start, entry.end, replacement));
            }
            continue;
        }
        if is_thinking(&entry.name) {
            if thinking_dirty {
                let replacement = if replaced_thinking {
                    String::new()
                } else {
                    replaced_thinking = true;
                    render_thinking(&draft.thinking_budget)?
                };
                edits.push((entry.start, entry.end, replacement));
            }
            continue;
        }
        if is_timeout(&entry.name) {
            if timeout_dirty {
                let replacement = if replaced_timeout {
                    String::new()
                } else {
                    replaced_timeout = true;
                    render_timeout(&draft.timeout_seconds)?
                };
                edits.push((entry.start, entry.end, replacement));
            }
        }
    }

    for entry in &entries {
        if entry.name != "providers" {
            continue;
        }
        let Some(label) = entry.label.as_deref() else {
            continue;
        };
        if let Some(index) = draft
            .providers
            .iter()
            .enumerate()
            .find(|(index, provider)| {
                !used[*index] && provider.original_label.as_deref() == Some(label)
            })
            .map(|(index, _)| index)
        {
            used[index] = true;
            if draft.providers[index].dirty {
                edits.push((
                    entry.start,
                    entry.end,
                    render_provider(&draft.providers[index]),
                ));
            }
        } else {
            edits.push((entry.start, entry.end, String::new()));
        }
    }

    let mut output = apply_edits(source, edits);
    for (index, provider) in draft.providers.iter().enumerate() {
        if !used[index] {
            output = append_entry(&output, &render_provider(provider));
        }
    }
    if default_dirty && !replaced_default && !draft.default_model.trim().is_empty() {
        output = append_entry(&output, &render_default(&draft.default_model));
    }
    if thinking_dirty && !replaced_thinking && !draft.thinking_budget.trim().is_empty() {
        output = append_entry(&output, &render_thinking(&draft.thinking_budget)?);
    }
    if timeout_dirty && !replaced_timeout && !draft.timeout_seconds.trim().is_empty() {
        output = append_entry(&output, &render_timeout(&draft.timeout_seconds)?);
    }
    Ok(normalize_final_newline(output))
}

fn render_provider(provider: &ProviderDraft) -> String {
    let mut block = provider.template.clone();
    block.name = "providers".to_string();
    block.labels = vec![provider.id.trim().to_string()];
    upsert(
        &mut block,
        &["apiKey", "api_key"],
        "apiKey",
        secret_value(&provider.api_key, provider.api_key_original.as_ref()),
    );
    upsert(
        &mut block,
        &["baseUrl", "base_url"],
        "baseUrl",
        secret_value(&provider.base_url, provider.base_url_original.as_ref()),
    );
    let extras = block
        .blocks
        .iter()
        .filter(|child| child.name != "models" && child.name != "model")
        .cloned()
        .collect::<Vec<_>>();
    block.blocks = extras;
    for model in &provider.models {
        block.blocks.push(render_model(model));
    }
    generate_acl(&Document {
        blocks: vec![block],
    })
}

fn render_model(model: &ModelDraft) -> Block {
    let mut block = model.template.clone();
    if block.name != "model" {
        block.name = "models".to_string();
    }
    block.labels = vec![model.id.trim().to_string()];
    upsert(
        &mut block,
        &["name"],
        "name",
        Some(Value::String(model.name.clone())),
    );
    upsert(
        &mut block,
        &["toolCall", "tool_call"],
        "toolCall",
        Some(Value::Bool(model.tool_call)),
    );
    upsert(
        &mut block,
        &["apiKey", "api_key"],
        "apiKey",
        secret_value(&model.api_key, model.api_key_original.as_ref()),
    );
    upsert(
        &mut block,
        &["baseUrl", "base_url"],
        "baseUrl",
        secret_value(&model.base_url, model.base_url_original.as_ref()),
    );
    set_limit(&mut block, model.context, model.output);
    block
}

fn secret_value(text: &str, original: Option<&OriginalValue>) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    if let Some(original) = original {
        if original.display == text {
            return Some(original.value.clone());
        }
    }
    Some(Value::String(text.to_string()))
}

fn render_default(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    render_assignment("default_model", Value::String(trimmed.to_string()))
}

fn render_thinking(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let budget = trimmed
        .parse::<u64>()
        .map_err(|_| "Reasoning budget must be a whole number of tokens".to_string())?;
    Ok(render_assignment(
        "thinking_budget",
        Value::Number(budget as f64),
    ))
}

fn render_timeout(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let ms = parse_timeout_ms(trimmed)?;
    Ok(render_assignment(
        "llm_api_timeout_ms",
        Value::Number(ms as f64),
    ))
}

fn render_assignment(name: &str, value: Value) -> String {
    let mut attributes = HashMap::new();
    attributes.insert(name.to_string(), value);
    generate_acl(&Document {
        blocks: vec![Block {
            name: name.to_string(),
            labels: Vec::new(),
            blocks: Vec::new(),
            attributes,
        }],
    })
}

fn parse_timeout_ms(text: &str) -> Result<u64, String> {
    let seconds: f64 = text
        .parse()
        .map_err(|_| "Timeout must be a number of seconds".to_string())?;
    if !(seconds.is_finite() && seconds > 0.0) {
        return Err("Timeout must be greater than 0 seconds".to_string());
    }
    Ok((seconds * 1000.0).round() as u64)
}

fn milliseconds_to_seconds(ms: f64) -> String {
    let seconds = ms / 1000.0;
    if (seconds - seconds.round()).abs() < 0.000_001 {
        format!("{}", seconds.round() as i64)
    } else {
        let text = format!("{seconds:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn upsert(block: &mut Block, keys: &[&str], canonical: &str, value: Option<Value>) {
    let existing = keys.iter().find(|key| block.attributes.contains_key(**key));
    let key = existing.copied().unwrap_or(canonical).to_string();
    for alias in keys {
        if *alias != key {
            block.attributes.remove(*alias);
        }
    }
    match value {
        Some(value) => {
            block.attributes.insert(key, value);
        }
        None => {
            block.attributes.remove(&key);
        }
    }
}

fn set_limit(block: &mut Block, context: u32, output: u32) {
    if context == 0 && output == 0 && !block.attributes.contains_key("limit") {
        return;
    }
    let mut pairs = match block.attributes.get("limit") {
        Some(Value::Object(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    upsert_pair(&mut pairs, "context", Value::Number(context as f64));
    upsert_pair(&mut pairs, "output", Value::Number(output as f64));
    block
        .attributes
        .insert("limit".to_string(), Value::Object(pairs));
}

fn upsert_pair(pairs: &mut Vec<(String, Value)>, key: &str, value: Value) {
    if let Some(existing) = pairs.iter_mut().find(|(name, _)| name == key) {
        existing.1 = value;
    } else {
        pairs.push((key.to_string(), value));
    }
}

fn original_attr(block: &Block, keys: &[&str]) -> Option<OriginalValue> {
    let value = keys.iter().find_map(|key| block.attributes.get(*key))?;
    Some(OriginalValue {
        display: value_display(value),
        value: value.clone(),
    })
}

fn value_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => {
            let mut attributes = HashMap::new();
            attributes.insert("v".to_string(), other.clone());
            let rendered = generate_acl(&Document {
                blocks: vec![Block {
                    name: "v".to_string(),
                    labels: Vec::new(),
                    blocks: Vec::new(),
                    attributes,
                }],
            });
            rendered
                .trim()
                .trim_start_matches("v = ")
                .trim()
                .to_string()
        }
    }
}

fn string_attr(block: &Block, keys: &[&str]) -> Option<String> {
    match keys.iter().find_map(|key| block.attributes.get(*key))? {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn bool_attr(block: &Block, keys: &[&str]) -> Option<bool> {
    match keys.iter().find_map(|key| block.attributes.get(*key))? {
        Value::Bool(value) => Some(*value),
        _ => None,
    }
}

fn number_text(block: &Block, keys: &[&str]) -> Option<String> {
    number_value(block, keys).map(|value| {
        if (value - value.round()).abs() < 0.000_001 {
            format!("{}", value.round() as i64)
        } else {
            value.to_string()
        }
    })
}

fn number_value(block: &Block, keys: &[&str]) -> Option<f64> {
    match keys.iter().find_map(|key| block.attributes.get(*key))? {
        Value::Number(value) => Some(*value),
        _ => None,
    }
}

fn limit_of(block: &Block) -> (u32, u32) {
    let Some(Value::Object(pairs)) = block.attributes.get("limit") else {
        return (0, 0);
    };
    let context = pair_number(pairs, "context");
    let output = pair_number(pairs, "output");
    (context, output)
}

fn pair_number(pairs: &[(String, Value)], key: &str) -> u32 {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| match value {
            Value::Number(number) if *number >= 0.0 => Some(*number as u32),
            _ => None,
        })
        .unwrap_or(0)
}

fn is_default_model(name: &str) -> bool {
    matches!(name, "default_model" | "defaultModel")
}

fn is_thinking(name: &str) -> bool {
    matches!(name, "thinking_budget" | "thinkingBudget")
}

fn is_timeout(name: &str) -> bool {
    matches!(
        name,
        "llm_api_timeout_ms" | "llmApiTimeoutMs" | "api_timeout_ms" | "model_api_timeout_ms"
    )
}

struct TopLevelEntry {
    name: String,
    label: Option<String>,
    start: usize,
    end: usize,
}

/// Same top-level scan as the core ACL section rewriter: comments and unrelated
/// blocks stay outside the ranges this editor replaces.
fn scan_top_level_entries(source: &str) -> Vec<TopLevelEntry> {
    let bytes = source.as_bytes();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        index = skip_space_and_comments(bytes, index);
        if index >= bytes.len() {
            break;
        }
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() && is_ident_part(bytes[index]) {
            index += 1;
        }
        let name = source[start..index].to_string();
        let header_start = index;
        let mut cursor = index;
        let mut braces = 0usize;
        let mut brackets = 0usize;
        let mut parens = 0usize;
        let mut quote = None;
        let mut escaped = false;
        let mut saw_delimiter = false;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if let Some(active_quote) = quote {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == active_quote {
                    quote = None;
                }
                cursor += 1;
                continue;
            }
            match byte {
                b'"' | b'\'' => {
                    quote = Some(byte);
                    cursor += 1;
                }
                b'#' => cursor = skip_line(bytes, cursor),
                b'/' if bytes.get(cursor + 1) == Some(&b'/') => cursor = skip_line(bytes, cursor),
                b'{' => {
                    saw_delimiter = true;
                    braces += 1;
                    cursor += 1;
                }
                b'}' => {
                    braces = braces.saturating_sub(1);
                    cursor += 1;
                }
                b'[' => {
                    saw_delimiter = true;
                    brackets += 1;
                    cursor += 1;
                }
                b']' => {
                    brackets = brackets.saturating_sub(1);
                    cursor += 1;
                }
                b'(' => {
                    saw_delimiter = true;
                    parens += 1;
                    cursor += 1;
                }
                b')' => {
                    parens = parens.saturating_sub(1);
                    cursor += 1;
                }
                b'=' | b':' => {
                    saw_delimiter = true;
                    cursor += 1;
                }
                b'\n' if saw_delimiter && braces == 0 && brackets == 0 && parens == 0 => {
                    cursor += 1;
                    break;
                }
                _ => cursor += 1,
            }
        }
        let header_end = source[header_start..cursor]
            .find('{')
            .or_else(|| source[header_start..cursor].find('='))
            .map(|offset| header_start + offset)
            .unwrap_or(cursor);
        entries.push(TopLevelEntry {
            name,
            label: parse_first_quoted(&source[header_start..header_end]),
            start,
            end: cursor,
        });
        index = cursor.max(start + 1);
    }
    entries
}

fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut output = source.to_string();
    for (start, end, replacement) in edits {
        let replacement = if replacement.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n", replacement.trim_end())
        };
        output.replace_range(start..end, &replacement);
    }
    output
}

fn append_entry(source: &str, entry: &str) -> String {
    let mut output = source.trim_end().to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(entry.trim());
    output.push('\n');
    output
}

fn normalize_final_newline(mut source: String) -> String {
    while source.ends_with("\n\n\n") {
        source.pop();
    }
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn skip_space_and_comments(bytes: &[u8], mut index: usize) -> usize {
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'#' {
            index = skip_line(bytes, index);
            continue;
        }
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'/') {
            index = skip_line(bytes, index);
            continue;
        }
        return index;
    }
}

fn skip_line(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn parse_first_quoted(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let start = bytes
        .iter()
        .position(|byte| *byte == b'"' || *byte == b'\'')?;
    let quote = bytes[start];
    let mut result = String::new();
    let mut escaped = false;
    for byte in &bytes[start + 1..] {
        if escaped {
            result.push(*byte as char);
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == quote {
            return Some(result);
        } else {
            result.push(*byte as char);
        }
    }
    None
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# keep this comment
default_model = "openai/my-model"

search {
  timeout = 20
}

providers "openai" {
  apiKey = env("OPENAI_API_KEY")
  baseUrl = "https://api.openai.com/v1/"
  models "my-model" {
    name = "My Model"
    toolCall = true
    limit = { context = 200000, output = 4096 }
  }
}
"#;

    #[test]
    fn load_reads_provider_model_and_default() {
        let draft = ConfigDraft::from_source(PathBuf::from("config.acl"), SAMPLE.to_string());
        assert!(draft.load_error.is_none());
        assert_eq!(draft.default_model, "openai/my-model");
        assert_eq!(draft.providers.len(), 1);
        assert_eq!(draft.providers[0].id, "openai");
        assert_eq!(draft.providers[0].api_key, "env(\"OPENAI_API_KEY\")");
        assert_eq!(draft.providers[0].base_url, "https://api.openai.com/v1/");
        assert_eq!(draft.providers[0].models[0].id, "my-model");
        assert_eq!(draft.providers[0].models[0].name, "My Model");
        assert_eq!(draft.providers[0].models[0].context, 200000);
        assert_eq!(draft.providers[0].models[0].output, 4096);
    }

    #[test]
    fn save_keeps_unedited_provider_and_other_blocks() {
        let dir = std::env::temp_dir().join(format!(
            "a3s-model-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.acl");
        let mut draft = ConfigDraft::from_source(path.clone(), SAMPLE.to_string());
        draft.providers.push(ProviderDraft {
            id: "extra".to_string(),
            original_label: None,
            api_key: "sk-test".to_string(),
            api_key_original: None,
            base_url: "https://example.test/v1/".to_string(),
            base_url_original: None,
            models: vec![ModelDraft {
                id: "extra-model".to_string(),
                name: "Extra".to_string(),
                context: 8192,
                output: 1024,
                tool_call: true,
                api_key: String::new(),
                api_key_original: None,
                base_url: String::new(),
                base_url_original: None,
                template: Block {
                    name: "models".to_string(),
                    labels: vec!["extra-model".to_string()],
                    blocks: Vec::new(),
                    attributes: HashMap::new(),
                },
            }],
            template: Block {
                name: "providers".to_string(),
                labels: vec!["extra".to_string()],
                blocks: Vec::new(),
                attributes: HashMap::new(),
            },
            dirty: true,
        });
        draft.default_model = "extra/extra-model".to_string();
        draft.save().expect("save");
        let saved = std::fs::read_to_string(&path).expect("read");
        assert!(saved.contains("# keep this comment"));
        assert!(saved.contains("search {"));
        assert!(saved.contains("timeout = 20"));
        assert!(saved.contains("apiKey = env(\"OPENAI_API_KEY\")"));
        assert!(saved.contains("providers \"extra\""));
        assert!(saved.contains("default_model = \"extra/extra-model\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edited_provider_keeps_env_call_when_the_key_text_is_unchanged() {
        let mut draft = ConfigDraft::from_source(PathBuf::from("config.acl"), SAMPLE.to_string());
        draft.providers[0].base_url = "https://changed.example/v1/".to_string();
        draft.providers[0].dirty = true;
        let saved = rewrite(&draft.source, &draft).expect("rewrite");
        assert!(saved.contains("env(\"OPENAI_API_KEY\")"));
        assert!(saved.contains("https://changed.example/v1/"));
        assert!(saved.contains("search {"));
    }

    #[test]
    fn model_connection_overrides_the_provider_and_blank_inherits() {
        let source = r#"
default_model = "openai/my-model"
providers "openai" {
  apiKey = env("OPENAI_API_KEY")
  baseUrl = "https://api.openai.com/v1/"
  models "my-model" {
    name = "My Model"
    apiKey = env("MODEL_KEY")
    baseUrl = "https://model.example/v1/"
  }
  models "other" {
    name = "Other"
  }
}
"#;
        let draft = ConfigDraft::from_source(PathBuf::from("config.acl"), source.to_string());
        assert_eq!(draft.providers[0].models[0].api_key, "env(\"MODEL_KEY\")");
        assert_eq!(
            draft.providers[0].models[0].base_url,
            "https://model.example/v1/"
        );
        assert!(draft.providers[0].models[1].api_key.is_empty());
        assert!(draft.providers[0].models[1].base_url.is_empty());

        let mut overridden = draft.clone();
        overridden.providers[0].models[1].api_key = "sk-other".to_string();
        overridden.providers[0].models[1].base_url = "https://other.example/v1/".to_string();
        overridden.providers[0].dirty = true;
        let written = rewrite(&overridden.source, &overridden).expect("rewrite");
        assert!(written.contains("apiKey = \"sk-other\""));
        assert!(written.contains("baseUrl = \"https://other.example/v1/\""));
        assert!(written.contains("apiKey = env(\"MODEL_KEY\")"));
        assert!(written.contains("apiKey = env(\"OPENAI_API_KEY\")"));

        let mut cleared = ConfigDraft::from_source(PathBuf::from("config.acl"), written);
        cleared.providers[0].models[0].api_key.clear();
        cleared.providers[0].models[0].base_url.clear();
        cleared.providers[0].dirty = true;
        let inherited = rewrite(&cleared.source, &cleared).expect("rewrite");
        let model = inherited
            .split("models \"my-model\"")
            .nth(1)
            .expect("model block");
        let model = model.split("models \"other\"").next().expect("model slice");
        assert!(!model.contains("apiKey"));
        assert!(!model.contains("baseUrl"));
        assert!(inherited.contains("apiKey = env(\"OPENAI_API_KEY\")"));
    }

    #[test]
    fn config_path_prefers_explicit_file() {
        let path = config_path_from(
            Some(std::ffi::OsStr::new("/tmp/custom.acl")),
            Some(std::ffi::OsStr::new("/home/user")),
            Some(std::ffi::OsStr::new("/home/user")),
            &|_| true,
        );
        assert_eq!(path, PathBuf::from("/tmp/custom.acl"));
    }
}
