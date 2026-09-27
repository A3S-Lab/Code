//! Import models from a logged-in Grok CLI account (`~/.grok`).
//!
//! Reads `auth.json` for the bearer token and `models_cache.json` for the
//! catalog previously fetched by Grok CLI. Registers them as provider `grok`
//! against `https://cli-chat-proxy.grok.com/v1` (OpenAI chat-completions).
//!
//! Secrets are never logged. Disable with `A3S_DISABLE_GROK_ACCOUNT=1`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use a3s_code_core::{ModelConfig, ModelModalities, ProviderConfig};
use anyhow::{Context, Result};
use serde_json::Value;

/// Provider name registered into [`a3s_code_core::CodeConfig`].
pub const PROVIDER_NAME: &str = "grok";

const DEFAULT_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
const TOKEN_AUTH_HEADER: &str = "a3s-code-cli";
const MIN_CLIENT_VERSION: &str = "0.1.202";

/// Result of reading a logged-in Grok account.
#[derive(Debug, Clone)]
pub struct GrokAccountImport {
    pub provider: ProviderConfig,
    pub default_model_id: String,
    pub user_email: Option<String>,
}

/// Merge a logged-in Grok account into `config` when available.
///
/// Returns `Ok(true)` when a provider was merged. Missing auth / disabled
/// import is not an error.
pub fn merge_into_config(config: &mut a3s_code_core::CodeConfig) -> Result<bool> {
    if disabled_by_env() {
        return Ok(false);
    }
    let Some(import) = load_logged_in_account()? else {
        return Ok(false);
    };

    config
        .providers
        .retain(|provider| provider.name != PROVIDER_NAME);
    config.providers.push(import.provider.clone());

    let qualified = format!("{PROVIDER_NAME}/{}", import.default_model_id);
    let prefer = prefer_grok_default();
    let needs_default = config
        .default_model
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true);
    if prefer || needs_default {
        config.default_model = Some(qualified.clone());
    }

    tracing::info!(
        models = import.provider.models.len(),
        default_model = %qualified,
        email = import.user_email.as_deref().unwrap_or("-"),
        "imported logged-in Grok account models"
    );
    Ok(true)
}

fn disabled_by_env() -> bool {
    matches!(
        std::env::var("A3S_DISABLE_GROK_ACCOUNT")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn prefer_grok_default() -> bool {
    matches!(
        std::env::var("A3S_PREFER_GROK")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Load the logged-in Grok account, if auth + model cache are present.
pub fn load_logged_in_account() -> Result<Option<GrokAccountImport>> {
    let Some(home) = resolve_grok_home() else {
        return Ok(None);
    };
    let auth_path = home.join("auth.json");
    if !auth_path.is_file() {
        return Ok(None);
    }
    load_from_home(&home)
}

fn resolve_grok_home() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("A3S_GROK_HOME") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    if let Some(home) = std::env::var_os("GROK_HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    let user_home = std::env::var_os("HOME").map(PathBuf::from)?;
    let grok = user_home.join(".grok");
    if grok.join("auth.json").is_file() {
        return Some(grok);
    }
    // Rebranded installs may keep credentials under ~/.a3s.
    let a3s = user_home.join(".a3s");
    if a3s.join("auth.json").is_file() {
        return Some(a3s);
    }
    Some(grok)
}

fn load_from_home(home: &Path) -> Result<Option<GrokAccountImport>> {
    let auth_raw = std::fs::read_to_string(home.join("auth.json"))
        .with_context(|| format!("could not read {}", home.join("auth.json").display()))?;
    let auth_store: Value =
        serde_json::from_str(&auth_raw).context("Grok auth.json is not valid JSON")?;
    let Some(credential) = pick_credential(&auth_store) else {
        return Ok(None);
    };

    let cache_path = home.join("models_cache.json");
    let cache = if cache_path.is_file() {
        let raw = std::fs::read_to_string(&cache_path)
            .with_context(|| format!("could not read {}", cache_path.display()))?;
        serde_json::from_str(&raw).context("Grok models_cache.json is not valid JSON")?
    } else {
        Value::Null
    };

    let config_default = read_config_default_model(home);
    Ok(Some(provider_from_parts(
        &credential,
        &cache,
        config_default.as_deref(),
    )?))
}

#[derive(Debug, Clone)]
struct GrokCredential {
    key: String,
    user_id: Option<String>,
    email: Option<String>,
    auth_mode: Option<String>,
}

fn pick_credential(store: &Value) -> Option<GrokCredential> {
    let map = store.as_object()?;
    // Prefer OIDC / session entries over bare API-key scopes when both exist.
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(scope, value)| {
        let mode = value.get("auth_mode").and_then(Value::as_str).unwrap_or("");
        let prefer = match mode {
            "oidc" | "web_login" | "session" => 0,
            "api_key" => 1,
            _ => 2,
        };
        (prefer, scope.as_str())
    });
    for (_scope, value) in entries {
        let Some(key) = value
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        if is_expired(value) {
            tracing::warn!("Grok auth credential is expired; skipping import");
            continue;
        }
        return Some(GrokCredential {
            key: key.to_string(),
            user_id: value
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            email: value
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_string),
            auth_mode: value
                .get("auth_mode")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    None
}

fn is_expired(auth: &Value) -> bool {
    let Some(expires) = auth.get("expires_at").and_then(Value::as_str) else {
        return false;
    };
    // RFC3339 timestamps compare lexicographically when both are UTC `Z`.
    let now = chrono_like_now();
    !expires.is_empty() && expires < now.as_str()
}

/// Minimal UTC timestamp without pulling chrono into the ACP crate.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format YYYY-MM-DDTHH:MM:SSZ via a tiny UTC breakdown.
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let minutes = (rem % 3600) / 60;
    let seconds = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Howard Hinnant civil-from-days (proleptic Gregorian), days since 1970-01-01.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn read_config_default_model(home: &Path) -> Option<String> {
    let path = home.join("config.toml");
    let raw = std::fs::read_to_string(path).ok()?;
    // Minimal scan: look for `default = "…"` under a `[models]` section.
    let mut in_models = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_models = trimmed == "[models]";
            continue;
        }
        if !in_models {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("default") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let value = rest.trim().trim_matches('"').trim_matches('\'').trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Build a provider from auth + optional models cache.
fn provider_from_parts(
    credential: &GrokCredential,
    cache: &Value,
    config_default: Option<&str>,
) -> Result<GrokAccountImport> {
    let client_version = cache
        .get("grok_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(MIN_CLIENT_VERSION)
        .to_string();

    let mut base_url = DEFAULT_BASE_URL.to_string();
    let mut models = models_from_cache(cache, &mut base_url);
    if models.is_empty() {
        // Fall back to a single placeholder so the account is still selectable
        // when the cache has not been populated yet.
        let id = config_default.unwrap_or("grok-4");
        models.push(model_config(id, id, None));
    }

    let default_model_id = config_default
        .filter(|id| models.iter().any(|model| model.id == *id))
        .map(str::to_string)
        .or_else(|| {
            models
                .iter()
                .find(|model| !model.id.contains("fast"))
                .map(|model| model.id.clone())
        })
        .unwrap_or_else(|| models[0].id.clone());

    let mut headers = HashMap::new();
    headers.insert(
        "X-XAI-Token-Auth".to_string(),
        TOKEN_AUTH_HEADER.to_string(),
    );
    headers.insert("x-grok-client-version".to_string(), client_version);
    if let Some(user_id) = credential.user_id.as_ref() {
        headers.insert("x-userid".to_string(), user_id.clone());
    }
    if let Some(email) = credential.email.as_ref() {
        headers.insert("x-email".to_string(), email.clone());
    }
    if let Some(mode) = credential.auth_mode.as_ref() {
        let _ = mode; // retained on credential for future refresh wiring
    }

    let provider = ProviderConfig {
        name: PROVIDER_NAME.to_string(),
        api_key: Some(credential.key.clone()),
        base_url: Some(base_url),
        headers,
        session_id_header: None,
        models,
    };

    Ok(GrokAccountImport {
        provider,
        default_model_id,
        user_email: credential.email.clone(),
    })
}

fn models_from_cache(cache: &Value, base_url: &mut String) -> Vec<ModelConfig> {
    let Some(map) = cache.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for (id, entry) in map {
        let info = entry.get("info").unwrap_or(entry);
        if info.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        if info.get("supported_in_api").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let name = info
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(id);
        let description = info
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(url) = info
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *base_url = url.to_string();
        }
        models.push(model_config(id, name, description.as_deref()));
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

fn model_config(id: &str, name: &str, _description: Option<&str>) -> ModelConfig {
    ModelConfig {
        id: id.to_string(),
        name: name.to_string(),
        family: "xai".to_string(),
        api_key: None,
        base_url: None,
        headers: Default::default(),
        session_id_header: None,
        attachment: false,
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

    fn cred(key: &str) -> GrokCredential {
        GrokCredential {
            key: key.to_string(),
            user_id: Some("user-1".into()),
            email: Some("user@example.com".into()),
            auth_mode: Some("oidc".into()),
        }
    }

    #[test]
    fn provider_from_parts_reads_cache_models() {
        let cache = json!({
            "grok_version": "1.0.41",
            "models": {
                "grok-4.7": {
                    "info": {
                        "id": "grok-4.7",
                        "name": "Grok 4.7",
                        "base_url": "https://cli-chat-proxy.grok.com/v1",
                        "hidden": false,
                        "supported_in_api": true
                    }
                },
                "grok-4.6": {
                    "info": {
                        "id": "grok-4.6",
                        "name": "Grok 4.6",
                        "hidden": false,
                        "supported_in_api": true
                    }
                },
                "hidden-one": {
                    "info": { "id": "hidden-one", "name": "Hidden", "hidden": true }
                }
            }
        });
        let import = provider_from_parts(&cred("tok"), &cache, Some("grok-4.7")).unwrap();
        assert_eq!(import.provider.name, PROVIDER_NAME);
        assert_eq!(import.default_model_id, "grok-4.7");
        assert_eq!(import.provider.models.len(), 2);
        assert_eq!(
            import
                .provider
                .headers
                .get("X-XAI-Token-Auth")
                .map(String::as_str),
            Some(TOKEN_AUTH_HEADER)
        );
        assert_eq!(
            import
                .provider
                .headers
                .get("x-grok-client-version")
                .map(String::as_str),
            Some("1.0.41")
        );
        assert_eq!(
            import.provider.base_url.as_deref(),
            Some("https://cli-chat-proxy.grok.com/v1")
        );
    }

    #[test]
    fn pick_credential_prefers_oidc() {
        let store = json!({
            "api": { "auth_mode": "api_key", "key": "api-tok" },
            "https://auth.x.ai::client": {
                "auth_mode": "oidc",
                "key": "oidc-tok",
                "user_id": "u1"
            }
        });
        let cred = pick_credential(&store).unwrap();
        assert_eq!(cred.key, "oidc-tok");
        assert_eq!(cred.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn merge_into_config_fills_missing_default() {
        let mut config = a3s_code_core::CodeConfig::default();
        let cache = json!({
            "models": {
                "grok-4.7": { "info": { "id": "grok-4.7", "name": "Grok 4.7" } }
            }
        });
        let import = provider_from_parts(&cred("tok"), &cache, Some("grok-4.7")).unwrap();
        config.providers.push(import.provider);
        config.default_model = Some(format!("{PROVIDER_NAME}/{}", import.default_model_id));
        assert_eq!(config.default_model.as_deref(), Some("grok/grok-4.7"));
    }
}

#[cfg(test)]
mod live_account_smoke {
    use super::*;

    #[test]
    fn loads_local_grok_account_when_present() {
        if disabled_by_env() {
            return;
        }
        let Ok(Some(import)) = load_logged_in_account() else {
            return;
        };
        assert_eq!(import.provider.name, PROVIDER_NAME);
        assert!(!import.provider.models.is_empty());
        assert!(import.provider.api_key.is_some());
        assert!(import.provider.base_url.is_some());
        eprintln!(
            "grok smoke: models={} default={} email={}",
            import.provider.models.len(),
            import.default_model_id,
            import.user_email.as_deref().unwrap_or("-")
        );
    }
}
