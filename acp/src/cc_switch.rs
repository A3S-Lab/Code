//! Import the current Claude provider from a local CC Switch database.
//!
//! CC Switch (`~/.cc-switch/cc-switch.db`) stores Anthropic-compatible upstream
//! credentials and model aliases used by Claude Code. When present, a3s-code-acp
//! merges that catalog as provider `cc-switch` so the TUI model picker can use
//! the same upstreams without duplicating keys in `~/.a3s/config.acl`.
//!
//! Secrets are never logged. Disable with `A3S_DISABLE_CC_SWITCH=1`.

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

/// Merge CC Switch's current Claude provider into `config` when available.
///
/// Returns `Ok(true)` when a provider was merged. Missing DB / disabled import
/// is not an error.
pub fn merge_into_config(config: &mut a3s_code_core::CodeConfig) -> Result<bool> {
    if disabled_by_env() {
        return Ok(false);
    }
    let Some(import) = load_current_claude()? else {
        return Ok(false);
    };

    config
        .providers
        .retain(|provider| provider.name != PROVIDER_NAME);
    config.providers.push(import.provider.clone());

    let qualified = format!("{PROVIDER_NAME}/{}", import.default_model_id);
    let prefer = prefer_cc_switch_default();
    let needs_default = config
        .default_model
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    if prefer || needs_default {
        config.default_model = Some(qualified.clone());
    }

    tracing::info!(
        provider = %import.provider_label,
        models = import.provider.models.len(),
        default_model = %qualified,
        via_proxy = import.via_proxy,
        "imported CC Switch Claude provider"
    );
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

/// Load the current Claude provider, if the CC Switch DB is present.
pub fn load_current_claude() -> Result<Option<CcSwitchImport>> {
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

fn load_current_claude_from_db(db_path: &Path) -> Result<Option<CcSwitchImport>> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("could not open CC Switch database {}", db_path.display()))?;

    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT name, settings_config
             FROM providers
             WHERE app_type = 'claude' AND is_current = 1
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("failed to query current Claude provider from CC Switch")?;

    let Some((provider_label, settings_raw)) = row else {
        return Ok(None);
    };

    let proxy = read_proxy_endpoint(&conn)?;
    let settings: Value = serde_json::from_str(&settings_raw)
        .context("CC Switch settings_config is not valid JSON")?;
    let import = provider_from_settings(
        &provider_label,
        &settings,
        proxy.as_ref().map(|endpoint| endpoint.base_url.as_str()),
    )?;
    Ok(Some(import))
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
pub(crate) fn provider_from_settings(
    provider_label: &str,
    settings: &Value,
    proxy_base_url: Option<&str>,
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

    let default_model_id = first_non_empty_env(env, &["ANTHROPIC_MODEL"])
        .map(|id| anthropic_model_id(&id))
        .filter(|id| models.iter().any(|model| model.id == *id))
        .unwrap_or_else(|| models[0].id.clone());

    let provider = ProviderConfig {
        name: PROVIDER_NAME.to_string(),
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

fn models_from_env(env: &serde_json::Map<String, Value>) -> Vec<ModelConfig> {
    // Map model id → display name using paired `*_MODEL` / `*_MODEL_NAME` keys.
    let mut id_names: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in env {
        let Some(stem) = key.strip_suffix("_MODEL_NAME") else {
            continue;
        };
        let Some(name) = value.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        let model_key = format!("{stem}_MODEL");
        if let Some(id) = env
            .get(&model_key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(anthropic_model_id)
            .filter(|id| !id.is_empty())
        {
            id_names.insert(id, name.to_string());
        }
    }

    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        push_env_model(env, key, &id_names, &mut seen, &mut models);
    }
    for (key, value) in env {
        if !key.ends_with("_MODEL") || key.ends_with("_MODEL_NAME") {
            continue;
        }
        let Some(id) = value
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(anthropic_model_id)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = id_names.get(&id).map(String::as_str).unwrap_or(id.as_str());
        models.push(model_config(&id, name));
    }
    models
}

fn push_env_model(
    env: &serde_json::Map<String, Value>,
    key: &str,
    id_names: &BTreeMap<String, String>,
    seen: &mut BTreeSet<String>,
    models: &mut Vec<ModelConfig>,
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
    if !seen.insert(id.clone()) {
        return;
    }
    let name = id_names.get(&id).map(String::as_str).unwrap_or(id.as_str());
    models.push(model_config(&id, name));
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
}

#[cfg(test)]
mod live_db_smoke {
    use super::*;

    #[test]
    fn loads_local_cc_switch_when_present() {
        if disabled_by_env() {
            return;
        }
        let Ok(Some(import)) = load_current_claude() else {
            return;
        };
        assert_eq!(import.provider.name, PROVIDER_NAME);
        assert!(!import.provider.models.is_empty());
        assert!(import.provider.api_key.is_some());
        assert!(import.provider.base_url.is_some());
        // Never assert on secret values.
        eprintln!(
            "cc-switch smoke: label={} models={} default={} via_proxy={}",
            import.provider_label,
            import.provider.models.len(),
            import.default_model_id,
            import.via_proxy
        );
    }
}
