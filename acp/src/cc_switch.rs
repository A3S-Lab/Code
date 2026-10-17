//! Import the current Claude provider from a local CC Switch database.
//!
//! CC Switch (`~/.cc-switch/cc-switch.db`) stores Anthropic-compatible upstream
//! credentials and model aliases used by Claude Code. When present, a3s-code-acp
//! merges that catalog as provider `cc-switch` so the TUI model picker can use
//! the same upstreams without duplicating keys in `~/.a3s/config.acl`.
//!
//! Runs only when the host opts in (`import_accounts = ["cc-switch"]` or
//! `--import-accounts cc-switch`). Secrets are never logged.
//! `A3S_DISABLE_CC_SWITCH=1` still turns it off.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use a3s_code_core::{ModelConfig, ModelModalities, ProviderConfig};
use anyhow::{anyhow, Context, Result};
use rusqlite::OptionalExtension;
use serde_json::Value;

/// Provider name registered into [`a3s_code_core::CodeConfig`].
pub const PROVIDER_NAME: &str = "cc-switch";

/// Result of reading the current Claude provider from CC Switch.
#[derive(Debug, Clone)]
pub struct CcSwitchImport {
    pub provider: ProviderConfig,
    /// Bare model id (without `cc-switch/` prefix).
    pub default_model_id: String,
    pub provider_label: String,
    pub via_proxy: bool,
}

/// One Claude profile from CC Switch, already turned into a core provider.
#[derive(Debug, Clone)]
pub struct CcSwitchCatalog {
    pub imports: Vec<CcSwitchImport>,
    /// `cc-switch/<model>` for the current profile, when that profile imported.
    pub default_qualified: Option<String>,
}

fn is_imported_cc_switch_provider(name: &str) -> bool {
    name == PROVIDER_NAME || name.starts_with("cc-switch-")
}

/// Merge every usable CC Switch Claude profile into `config`.
///
/// Returns `Ok(true)` when at least one profile was merged. A missing DB or a
/// disabled import is not an error. A profile that has no token or no base URL
/// is skipped and logged. It does not drop the other profiles.
pub fn merge_into_config(config: &mut a3s_code_core::CodeConfig) -> Result<bool> {
    if disabled_by_env() {
        return Ok(false);
    }
    let Some(catalog) = load_current_claude()? else {
        return Ok(false);
    };

    config
        .providers
        .retain(|provider| !is_imported_cc_switch_provider(&provider.name));

    let prefer = prefer_cc_switch_default();
    let needs_default = config
        .default_model
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    if prefer || needs_default {
        if let Some(qualified) = catalog.default_qualified.clone() {
            config.default_model = Some(qualified);
        }
    }

    let providers = catalog.imports.len();
    let models: usize = catalog
        .imports
        .iter()
        .map(|import| import.provider.models.len())
        .sum();
    for import in &catalog.imports {
        tracing::info!(
            provider = %import.provider_label,
            provider_name = %import.provider.name,
            models = import.provider.models.len(),
            via_proxy = import.via_proxy,
            "imported CC Switch Claude provider"
        );
    }
    tracing::info!(
        providers,
        models,
        default_model = catalog.default_qualified.as_deref().unwrap_or(""),
        "imported CC Switch Claude catalog"
    );
    config
        .providers
        .extend(catalog.imports.into_iter().map(|import| import.provider));
    Ok(true)
}

fn disabled_by_env() -> bool {
    matches!(
        std::env::var("A3S_DISABLE_CC_SWITCH")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn prefer_cc_switch_default() -> bool {
    matches!(
        std::env::var("A3S_PREFER_CC_SWITCH")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Load every usable Claude profile, if the CC Switch DB is present.
pub fn load_current_claude() -> Result<Option<CcSwitchCatalog>> {
    let Some(db_path) = resolve_db_path() else {
        return Ok(None);
    };
    if !db_path.is_file() {
        return Ok(None);
    }
    load_current_claude_from_db(&db_path)
}

fn resolve_db_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("A3S_CC_SWITCH_DB") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    if let Some(home) = std::env::var_os("CC_SWITCH_HOME") {
        let path = PathBuf::from(home).join("cc-switch.db");
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".cc-switch/cc-switch.db"))
}

struct ClaudeProviderRow {
    label: String,
    settings: Value,
    is_current: bool,
}

fn load_current_claude_from_db(db_path: &Path) -> Result<Option<CcSwitchCatalog>> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("could not open CC Switch database {}", db_path.display()))?;

    let mut statement = conn
        .prepare(
            "SELECT name, settings_config, COALESCE(is_current, 0)
             FROM providers
             WHERE app_type = 'claude'
             ORDER BY COALESCE(is_current, 0) DESC, name",
        )
        .context("failed to query Claude providers from CC Switch")?;
    let scanned = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .context("failed to read Claude providers from CC Switch")?;

    let mut rows = Vec::new();
    for row in scanned {
        let (label, settings_raw, is_current) = row.context("failed to read a CC Switch row")?;
        let settings = match serde_json::from_str(&settings_raw) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::error!(
                    provider = %label,
                    error = %error,
                    "CC Switch provider settings are not JSON"
                );
                continue;
            }
        };
        rows.push(ClaudeProviderRow {
            label,
            settings,
            is_current: is_current != 0,
        });
    }
    let proxy = read_proxy_endpoint(&conn)?;
    let catalog = catalog_from_rows(
        rows,
        proxy.as_ref().map(|endpoint| endpoint.base_url.as_str()),
    );
    if catalog.imports.is_empty() {
        Ok(None)
    } else {
        Ok(Some(catalog))
    }
}

/// The local CC Switch proxy, when enabled, fronts only the current profile.
/// Other profiles keep the upstream base URL stored on that profile.
fn catalog_from_rows(
    rows: Vec<ClaudeProviderRow>,
    proxy_base_url: Option<&str>,
) -> CcSwitchCatalog {
    let mut used_names = BTreeSet::new();
    used_names.insert(PROVIDER_NAME.to_string());
    let mut imports = Vec::new();
    let mut default_qualified = None;
    let mut saw_current = false;

    for row in rows {
        let current = row.is_current && !saw_current;
        let provider_name;
        let picker_prefix;
        let proxy_url;
        if current {
            saw_current = true;
            provider_name = PROVIDER_NAME.to_string();
            picker_prefix = None;
            proxy_url = proxy_base_url;
        } else {
            provider_name = unique_provider_name(&provider_slug(&row.label), &mut used_names);
            picker_prefix = Some(row.label.as_str());
            proxy_url = None;
        }
        match provider_from_settings_as(
            &row.label,
            &row.settings,
            proxy_url,
            &provider_name,
            picker_prefix,
        ) {
            Ok(import) => {
                if current {
                    default_qualified =
                        Some(format!("{PROVIDER_NAME}/{}", import.default_model_id));
                }
                imports.push(import);
            }
            Err(error) => {
                tracing::error!(
                    provider = %row.label,
                    error = %error,
                    "skipped CC Switch Claude provider"
                );
            }
        }
    }

    CcSwitchCatalog {
        imports,
        default_qualified,
    }
}

fn provider_slug(label: &str) -> String {
    let mut slug = String::new();
    let mut hyphen = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            hyphen = false;
        } else if !slug.is_empty() && !hyphen {
            slug.push('-');
            hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "provider".to_string()
    } else {
        slug
    }
}

fn unique_provider_name(slug: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = format!("cc-switch-{slug}");
    let mut suffix = 2u32;
    while used.contains(&name) {
        name = format!("cc-switch-{slug}-{suffix}");
        suffix += 1;
    }
    used.insert(name.clone());
    name
}

#[derive(Debug, Clone)]
struct ProxyEndpoint {
    base_url: String,
}

fn read_proxy_endpoint(conn: &rusqlite::Connection) -> Result<Option<ProxyEndpoint>> {
    let row: Option<(i64, i64, String, i64)> = conn
        .query_row(
            "SELECT COALESCE(proxy_enabled, 0), COALESCE(enabled, 0),
                    listen_address, listen_port
             FROM proxy_config
             WHERE app_type = 'claude'
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .context("failed to query CC Switch Claude proxy_config")?;

    let Some((proxy_enabled, enabled, address, port)) = row else {
        return Ok(None);
    };
    if proxy_enabled == 0 && enabled == 0 {
        return Ok(None);
    }
    if address.trim().is_empty() || port <= 0 {
        return Ok(None);
    }
    Ok(Some(ProxyEndpoint {
        base_url: format!("http://{address}:{port}"),
    }))
}

/// Build a provider from a CC Switch `settings_config` document.
///
/// The provider name is `cc-switch`. Call [`provider_from_settings_as`] when
/// the profile is not the current one.
#[allow(dead_code)] // profile-picker surface; lands with its slice
pub(crate) fn provider_from_settings(
    provider_label: &str,
    settings: &Value,
    proxy_base_url: Option<&str>,
) -> Result<CcSwitchImport> {
    provider_from_settings_as(
        provider_label,
        settings,
        proxy_base_url,
        PROVIDER_NAME,
        None,
    )
}

fn provider_from_settings_as(
    provider_label: &str,
    settings: &Value,
    proxy_base_url: Option<&str>,
    provider_name: &str,
    picker_prefix: Option<&str>,
) -> Result<CcSwitchImport> {
    let env = settings
        .get("env")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("CC Switch provider is missing env settings"))?;

    let api_key = first_non_empty_env(
        env,
        &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_API_TOKEN",
        ],
    )
    .ok_or_else(|| anyhow!("CC Switch Claude provider has no API token"))?;

    let upstream_base = first_non_empty_env(env, &["ANTHROPIC_BASE_URL", "ANTHROPIC_BASE_URI"]);
    let via_proxy = proxy_base_url.is_some();
    let base_url = proxy_base_url
        .map(str::to_string)
        .or(upstream_base)
        .ok_or_else(|| anyhow!("CC Switch Claude provider has no base URL"))?;

    let mut models = models_from_catalog_array(settings);
    if models.is_empty() {
        models = models_from_env(env);
    }
    if models.is_empty() {
        return Err(anyhow!("CC Switch Claude provider lists no models"));
    }

    for model in &mut models {
        model.api_key = Some(api_key.clone());
        model.base_url = Some(base_url.clone());
        if let Some(prefix) = picker_prefix {
            model.name = format!("{prefix} · {}", model.name);
        }
    }

    let default_model_id = first_non_empty_env(env, &["ANTHROPIC_MODEL"])
        .map(|id| anthropic_model_id(&id))
        .filter(|id| models.iter().any(|model| model.id == *id))
        .unwrap_or_else(|| models[0].id.clone());

    let provider = ProviderConfig {
        name: provider_name.to_string(),
        api_key: Some(api_key),
        base_url: Some(base_url),
        headers: Default::default(),
        session_id_header: None,
        models,
    };

    Ok(CcSwitchImport {
        provider,
        default_model_id,
        provider_label: provider_label.to_string(),
        via_proxy,
    })
}

fn first_non_empty_env(env: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = env.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn models_from_catalog_array(settings: &Value) -> Vec<ModelConfig> {
    let Some(items) = settings.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for item in items {
        let id = item
            .get("id")
            .or_else(|| item.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(anthropic_model_id)
            .filter(|id| !id.is_empty());
        let Some(id) = id else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id.as_str())
            .to_string();
        models.push(model_config(&id, &name));
    }
    models
}

const ENV_MODEL_KEYS: [&str; 6] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];

fn models_from_env(env: &serde_json::Map<String, Value>) -> Vec<ModelConfig> {
    let mut slots: Vec<(String, Option<String>)> = Vec::new();
    for key in ENV_MODEL_KEYS {
        push_env_slot(env, key, &mut slots);
    }
    let mut extra: Vec<&str> = env
        .keys()
        .map(String::as_str)
        .filter(|key| key.ends_with("_MODEL") && !key.ends_with("_MODEL_NAME"))
        .filter(|key| !ENV_MODEL_KEYS.contains(key))
        .collect();
    extra.sort_unstable();
    for key in extra {
        push_env_slot(env, key, &mut slots);
    }

    let mut order = Vec::new();
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, name) in slots {
        if !order.contains(&id) {
            order.push(id.clone());
        }
        if let Some(name) = name {
            names.entry(id).or_default().push(name);
        }
    }
    order
        .into_iter()
        .map(|id| {
            let display =
                choose_display_name(&id, names.get(&id).map(Vec::as_slice).unwrap_or(&[]));
            model_config(&id, &display)
        })
        .collect()
}

fn push_env_slot(
    env: &serde_json::Map<String, Value>,
    key: &str,
    slots: &mut Vec<(String, Option<String>)>,
) {
    let Some(id) = env
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(anthropic_model_id)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let stem = key.strip_suffix("_MODEL").unwrap_or(key);
    let name = env
        .get(&format!("{stem}_MODEL_NAME"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    slots.push((id, name));
}

/// Several Claude Code slots can share one API id while their display names differ.
/// Prefer the name that is the API id. Otherwise keep the first name.
fn choose_display_name(id: &str, names: &[String]) -> String {
    if names.iter().any(|name| name == id) {
        return id.to_string();
    }
    names.first().cloned().unwrap_or_else(|| id.to_string())
}

/// Model id sent to an Anthropic-compatible `/v1/messages` endpoint.
///
/// CC Switch stores Claude Code slot ids with a context hint such as
/// `glm-5.3-flash[1M]`. The upstream model code is the bare id; the hint is
/// not part of the API model name.
fn anthropic_model_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(open) = trimmed.rfind('[') else {
        return trimmed.to_string();
    };
    if open == 0 || !trimmed.ends_with(']') {
        return trimmed.to_string();
    }
    let hint = &trimmed[open + 1..trimmed.len() - 1];
    if !is_context_window_hint(hint) {
        return trimmed.to_string();
    }
    trimmed[..open].trim().to_string()
}

fn is_context_window_hint(hint: &str) -> bool {
    let hint = hint.trim();
    let mut chars = hint.chars();
    let Some(unit) = chars.next_back() else {
        return false;
    };
    if !matches!(unit, 'k' | 'K' | 'm' | 'M') {
        return false;
    }
    let number = chars.as_str();
    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
}

fn model_config(id: &str, name: &str) -> ModelConfig {
    ModelConfig {
        id: id.to_string(),
        name: name.to_string(),
        family: String::new(),
        api_key: None,
        base_url: None,
        headers: Default::default(),
        session_id_header: None,
        attachment: false,
        // Claude / Anthropic-compatible models accept thinking budgets via a3s-code.
        reasoning: true,
        tool_call: true,
        temperature: true,
        release_date: None,
        modalities: ModelModalities::default(),
        cost: Default::default(),
        limit: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_from_settings_reads_env_models() {
        let settings = json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-test",
                "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic",
                "ANTHROPIC_MODEL": "glm-5.3-flash[1M]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-5.2",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "glm-5.2",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.3-flash[1M]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "glm-5.3-flash",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.3-flashx[1M]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "glm-5.3-flashx"
            }
        });
        let import = provider_from_settings("Zhipu GLM 01", &settings, None).unwrap();
        assert_eq!(import.provider.name, PROVIDER_NAME);
        assert_eq!(import.default_model_id, "glm-5.3-flash");
        assert_eq!(
            import.provider.base_url.as_deref(),
            Some("https://open.bigmodel.cn/api/anthropic")
        );
        let ids: Vec<_> = import
            .provider
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert!(ids.contains(&"glm-5.2"));
        assert!(ids.contains(&"glm-5.3-flash"));
        assert!(ids.contains(&"glm-5.3-flashx"));
        assert!(!ids.iter().any(|id| id.contains('[')));
        let sonnet = import
            .provider
            .models
            .iter()
            .find(|model| model.id == "glm-5.3-flash")
            .unwrap();
        assert_eq!(sonnet.name, "glm-5.3-flash");
    }

    #[test]
    fn context_window_suffix_is_not_part_of_the_api_model_id() {
        assert_eq!(anthropic_model_id("glm-5.3[1M]"), "glm-5.3");
        assert_eq!(anthropic_model_id("glm-5.2"), "glm-5.2");
        assert_eq!(
            anthropic_model_id("claude-sonnet-4-5[200k]"),
            "claude-sonnet-4-5"
        );
        assert_eq!(anthropic_model_id("custom[beta]"), "custom[beta]");
    }

    #[test]
    fn provider_from_settings_prefers_local_proxy() {
        let settings = json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-test",
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com",
                "ANTHROPIC_MODEL": "claude-sonnet-4"
            }
        });
        let import =
            provider_from_settings("ClaudeAPI", &settings, Some("http://127.0.0.1:15721")).unwrap();
        assert!(import.via_proxy);
        assert_eq!(
            import.provider.base_url.as_deref(),
            Some("http://127.0.0.1:15721")
        );
    }

    #[test]
    fn merge_into_config_fills_missing_default() {
        let mut config = a3s_code_core::CodeConfig::default();
        let settings = json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-test",
                "ANTHROPIC_BASE_URL": "https://example.test",
                "ANTHROPIC_MODEL": "demo-model"
            }
        });
        let import = provider_from_settings("Demo", &settings, None).unwrap();
        config.providers.push(import.provider);
        config.default_model = Some(format!("{PROVIDER_NAME}/{}", import.default_model_id));
        assert_eq!(
            config.default_model.as_deref(),
            Some("cc-switch/demo-model")
        );
    }

    fn glm03_slots() -> Value {
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-current",
                "ANTHROPIC_BASE_URL": "https://current.example/anthropic",
                "ANTHROPIC_DEFAULT_FABLE_MODEL": "glm-5.3[1M]",
                "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME": "glm-5.3",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-5.3",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "glm-5.2",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.3[1M]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "glm-5.3-flashx",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.3[1M]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "glm-5.3-flash",
                "ANTHROPIC_MODEL": "glm-5.3-flash[1M]",
                "CLAUDE_CODE_SUBAGENT_MODEL": "glm-5.3[1M]"
            }
        })
    }

    #[test]
    fn collapsed_slots_keep_two_ids_and_the_matching_name() {
        let import = provider_from_settings("Zhipu GLM 03", &glm03_slots(), None).unwrap();
        let listed: Vec<_> = import
            .provider
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect();
        assert_eq!(
            listed,
            vec![("glm-5.3-flash", "glm-5.3-flash"), ("glm-5.3", "glm-5.3")]
        );
        assert_eq!(import.default_model_id, "glm-5.3-flash");
    }

    #[test]
    fn two_providers_keep_the_same_model_id() {
        let other = json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "sk-other",
                "ANTHROPIC_BASE_URL": "https://other.example/anthropic",
                "ANTHROPIC_MODEL": "glm-5.3",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-5.2",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "glm-5.2",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.3-flashx",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "glm-5.3-flashx"
            }
        });
        let catalog = catalog_from_rows(
            vec![
                ClaudeProviderRow {
                    label: "Zhipu GLM 03".into(),
                    settings: glm03_slots(),
                    is_current: true,
                },
                ClaudeProviderRow {
                    label: "Zhipu GLM 01".into(),
                    settings: other,
                    is_current: false,
                },
            ],
            None,
        );
        assert_eq!(
            catalog.default_qualified.as_deref(),
            Some("cc-switch/glm-5.3-flash")
        );
        let current = catalog
            .imports
            .iter()
            .find(|import| import.provider.name == PROVIDER_NAME)
            .unwrap();
        let extra = catalog
            .imports
            .iter()
            .find(|import| import.provider.name == "cc-switch-zhipu-glm-01")
            .unwrap();
        assert!(current
            .provider
            .models
            .iter()
            .any(|model| model.id == "glm-5.3"));
        assert!(extra
            .provider
            .models
            .iter()
            .any(|model| model.id == "glm-5.3"));
        assert_eq!(
            current.provider.base_url.as_deref(),
            Some("https://current.example/anthropic")
        );
        assert_eq!(
            extra.provider.base_url.as_deref(),
            Some("https://other.example/anthropic")
        );
        let shared = extra
            .provider
            .models
            .iter()
            .find(|model| model.id == "glm-5.3")
            .unwrap();
        assert_eq!(
            shared.base_url.as_deref(),
            Some("https://other.example/anthropic")
        );
        assert_eq!(shared.api_key.as_deref(), Some("sk-other"));
        assert_eq!(shared.name, "Zhipu GLM 01 · glm-5.3");
    }

    #[test]
    fn provider_without_token_is_skipped() {
        let broken = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://broken.example/anthropic",
                "ANTHROPIC_MODEL": "glm-5.3"
            }
        });
        let catalog = catalog_from_rows(
            vec![
                ClaudeProviderRow {
                    label: "Broken".into(),
                    settings: broken,
                    is_current: false,
                },
                ClaudeProviderRow {
                    label: "Zhipu GLM 03".into(),
                    settings: glm03_slots(),
                    is_current: true,
                },
            ],
            None,
        );
        assert_eq!(catalog.imports.len(), 1);
        assert_eq!(catalog.imports[0].provider.name, PROVIDER_NAME);
        assert_eq!(
            catalog.default_qualified.as_deref(),
            Some("cc-switch/glm-5.3-flash")
        );
    }
}

#[cfg(test)]
mod live_db_smoke {
    use super::*;

    #[test]
    fn loads_local_cc_switch_when_present() {
        if disabled_by_env() {
            return;
        }
        let Ok(Some(catalog)) = load_current_claude() else {
            return;
        };
        assert!(catalog
            .imports
            .iter()
            .any(|import| import.provider.name == PROVIDER_NAME));
        assert!(catalog
            .imports
            .iter()
            .all(|import| !import.provider.models.is_empty()));
        assert!(catalog
            .imports
            .iter()
            .all(|import| import.provider.api_key.is_some() && import.provider.base_url.is_some()));
        // Never print token or base URL values.
        eprintln!(
            "cc-switch smoke: providers={} models={} default_set={}",
            catalog.imports.len(),
            catalog
                .imports
                .iter()
                .map(|import| import.provider.models.len())
                .sum::<usize>(),
            catalog.default_qualified.is_some()
        );
    }
}
