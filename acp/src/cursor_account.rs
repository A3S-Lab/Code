//! Import models from the Cursor account already logged in on this machine.
//!
//! The catalog is the CLI model list (`agent models`). The bearer stays in the
//! in-memory provider API key. Nothing here rewrites ACL. Disable with
//! `A3S_DISABLE_CURSOR_ACCOUNT=1`. Prefer the account as the default model
//! with `A3S_PREFER_CURSOR=1` (same missing-default rule as the Grok import).
//!
//! Fixture tests set `A3S_CURSOR_HOME`, `A3S_CURSOR_TOKEN_FILE`, and
//! `A3S_CURSOR_MODELS_FILE`. Those paths never read the keychain, the IDE
//! database, or the network, and they do not spawn `agent`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use a3s_code_core::{ModelConfig, ModelModalities, ProviderConfig};
use anyhow::{Context, Result};
use serde_json::Value;

/// Provider name registered into [`a3s_code_core::CodeConfig`].
pub const PROVIDER_NAME: &str = "cursor";

/// Account endpoint. `CURSOR_API_ENDPOINT` may replace the origin.
pub const DEFAULT_ENDPOINT: &str = "https://api2.cursor.sh";

const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_CATALOG_BYTES: usize = 1024 * 1024;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// Read-only catalog invocation. No prompt, no print mode, no write approval.
pub fn catalog_list_args() -> &'static [&'static str] {
    &["models"]
}

/// Result of reading a logged-in Cursor account. Not `Debug`: the provider
/// API key is the bearer.
pub struct CursorAccountImport {
    pub provider: ProviderConfig,
    pub default_model_id: String,
}

/// Merge a logged-in Cursor account into `config` when one is available.
///
/// Returns `Ok(true)` when a provider was merged. A missing login, an
/// unusable credential, an empty catalog, or the disable switch is `Ok(false)`.
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
    let prefer = prefer_cursor_default();
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
        "imported logged-in Cursor account models"
    );
    Ok(true)
}

fn disabled_by_env() -> bool {
    env_flag("A3S_DISABLE_CURSOR_ACCOUNT")
}

fn prefer_cursor_default() -> bool {
    env_flag("A3S_PREFER_CURSOR")
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Load the logged-in Cursor account when identity, a usable bearer, and a
/// non-empty model list are all present.
pub fn load_logged_in_account() -> Result<Option<CursorAccountImport>> {
    let Some(home) = resolve_cursor_home() else {
        return Ok(None);
    };
    let config_path = home.join("cli-config.json");
    if !config_path.is_file() {
        return Ok(None);
    }
    let raw = read_bounded(&config_path, MAX_CONFIG_BYTES)
        .with_context(|| format!("could not read {}", config_path.display()))?;
    let Some(raw) = raw else {
        tracing::warn!("cursor cli-config exceeded the read bound; skipping import");
        return Ok(None);
    };
    let config: Value =
        serde_json::from_str(&raw).context("Cursor cli-config.json is not valid JSON")?;
    let Some(identity) = account_identity(&config) else {
        return Ok(None);
    };

    let Some(token) = load_usable_token(&identity)? else {
        return Ok(None);
    };
    let Some(catalog_text) = read_model_catalog()? else {
        return Ok(None);
    };
    let models = parse_model_catalog(&catalog_text)?;
    if models.is_empty() {
        return Ok(None);
    }

    let default_model_id = models
        .iter()
        .find(|model| !is_placeholder_model(&model.id))
        .map(|model| model.id.clone())
        .unwrap_or_else(|| models[0].id.clone());

    let mut headers = std::collections::HashMap::new();
    headers.insert("x-cursor-client-type".to_string(), "cli".to_string());
    headers.insert("x-cursor-client-version".to_string(), client_version());
    headers.insert(
        "x-ghost-mode".to_string(),
        if ghost_mode(&config) { "true" } else { "false" }.to_string(),
    );

    let provider = ProviderConfig {
        name: PROVIDER_NAME.to_string(),
        api_key: Some(token),
        base_url: Some(account_endpoint()),
        headers,
        session_id_header: None,
        models,
    };
    Ok(Some(CursorAccountImport {
        provider,
        default_model_id,
    }))
}

struct AccountIdentity {
    email: Option<String>,
    auth_id: Option<String>,
}

fn account_identity(config: &Value) -> Option<AccountIdentity> {
    let info = config.get("authInfo")?.as_object()?;
    let email = info
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let auth_id = info
        .get("authId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let user_id = info
        .get("userId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let display = info
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if email.is_none() && auth_id.is_none() && user_id.is_none() && display.is_none() {
        return None;
    }
    Some(AccountIdentity { email, auth_id })
}

fn resolve_cursor_home() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("A3S_CURSOR_HOME") {
        if explicit.is_empty() {
            return None;
        }
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")?;
    let cursor = PathBuf::from(home).join(".cursor");
    if cursor.join("cli-config.json").is_file() {
        Some(cursor)
    } else {
        None
    }
}

fn fixture_mode() -> bool {
    env_nonempty("A3S_CURSOR_HOME")
        || env_nonempty("A3S_CURSOR_TOKEN_FILE")
        || env_nonempty("A3S_CURSOR_MODELS_FILE")
        || env_nonempty("A3S_CURSOR_STATE_DB")
}

fn env_nonempty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn load_usable_token(identity: &AccountIdentity) -> Result<Option<String>> {
    let cli_token = read_cli_token()?;
    if let Some(token) = cli_token {
        if token_usable(&token) {
            note_ide_account_mismatch(identity.email.as_deref());
            return Ok(Some(token));
        }
        tracing::warn!("cursor access token is unusable; not sending it");
    }
    Ok(ide_token_when_same_account(identity))
}

fn read_cli_token() -> Result<Option<String>> {
    if let Some(explicit) = std::env::var_os("A3S_CURSOR_TOKEN_FILE") {
        if explicit.is_empty() {
            return Ok(None);
        }
        return read_token_file(&PathBuf::from(explicit));
    }
    Ok(read_keychain_token())
}

fn read_token_file(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let Some(raw) = read_bounded(path, MAX_SECRET_BYTES)? else {
        tracing::warn!("cursor token file exceeded the read bound; skipping import");
        return Ok(None);
    };
    let token = raw.trim();
    if token.is_empty() {
        return Ok(None);
    }
    Ok(Some(token.to_string()))
}

fn read_keychain_token() -> Option<String> {
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            "cursor-access-token",
            "-a",
            "cursor-user",
            "-w",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > MAX_SECRET_BYTES {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn note_ide_account_mismatch(cli_email: Option<&str>) {
    let Some(cli_email) = cli_email else {
        return;
    };
    let Some(path) = ide_state_db() else {
        return;
    };
    let Ok(Some(ide_email)) = read_ide_item(&path, "cursorAuth/cachedEmail") else {
        return;
    };
    if !ide_email.eq_ignore_ascii_case(cli_email) {
        tracing::warn!(
            "Cursor IDE login belongs to a different account than the CLI session; the IDE token was not used"
        );
    }
}

fn ide_token_when_same_account(identity: &AccountIdentity) -> Option<String> {
    let path = ide_state_db()?;
    let ide_email = read_ide_item(&path, "cursorAuth/cachedEmail")
        .ok()
        .flatten();
    let token = read_ide_item(&path, "cursorAuth/accessToken")
        .ok()
        .flatten()?;
    if !token_usable(&token) {
        return None;
    }
    let emails_differ = match (identity.email.as_deref(), ide_email.as_deref()) {
        (Some(cli), Some(ide)) => !cli.eq_ignore_ascii_case(ide),
        _ => false,
    };
    let subs_differ = match (identity.auth_id.as_deref(), jwt_string_claim(&token, "sub")) {
        (Some(auth_id), Some(sub)) => auth_id != sub,
        _ => false,
    };
    if emails_differ || subs_differ {
        tracing::warn!(
            "Cursor IDE login belongs to a different account than the CLI session; the IDE token was not used"
        );
        return None;
    }
    let emails_match = match (identity.email.as_deref(), ide_email.as_deref()) {
        (Some(cli), Some(ide)) => cli.eq_ignore_ascii_case(ide),
        _ => false,
    };
    let subs_match = match (identity.auth_id.as_deref(), jwt_string_claim(&token, "sub")) {
        (Some(auth_id), Some(sub)) => auth_id == sub,
        _ => false,
    };
    if emails_match || subs_match {
        return Some(token);
    }
    tracing::warn!(
        "Cursor IDE identity could not be compared with the CLI session; the IDE token was not used"
    );
    None
}

fn ide_state_db() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("A3S_CURSOR_STATE_DB") {
        if explicit.is_empty() {
            return None;
        }
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    if fixture_mode() {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home)
        .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    path.is_file().then_some(path)
}

fn read_ide_item(path: &Path, key: &str) -> Result<Option<String>> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("could not open Cursor state database {}", path.display()))?;
    let mut statement = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .context("failed to query Cursor ItemTable")?;
    let mut rows = statement
        .query(rusqlite::params![key])
        .context("failed to read a Cursor ItemTable row")?;
    let Some(row) = rows
        .next()
        .context("failed to step a Cursor ItemTable row")?
    else {
        return Ok(None);
    };
    let value: String = match row.get(0) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.len() > MAX_SECRET_BYTES {
        tracing::warn!("cursor IDE auth value exceeded the read bound");
        return Ok(None);
    }
    Ok(Some(value))
}

fn token_usable(token: &str) -> bool {
    match jwt_exp(token) {
        None => true,
        Some(None) => false,
        Some(Some(exp)) => exp > unix_now(),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// `None` when `token` is not a JWT. `Some(None)` when it is a JWT with no
/// numeric `exp`. `Some(Some(exp))` when `exp` is present.
fn jwt_exp(token: &str) -> Option<Option<i64>> {
    let payload = jwt_payload(token)?;
    match payload.get("exp") {
        Some(Value::Number(number)) => Some(number.as_i64()),
        _ => Some(None),
    }
}

fn jwt_string_claim(token: &str, claim: &str) -> Option<String> {
    jwt_payload(token)?
        .get(claim)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let header = parts.next()?;
    let payload = parts.next()?;
    let signature = parts.next()?;
    if parts.next().is_some() || header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let bytes = decode_base64url(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let mut filtered = Vec::with_capacity(input.len());
    for byte in input.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        filtered.push(value(byte)?);
    }
    let mut out = Vec::with_capacity(filtered.len() * 3 / 4);
    let mut index = 0;
    while index + 1 < filtered.len() {
        let b0 = filtered[index];
        let b1 = filtered[index + 1];
        out.push((b0 << 2) | (b1 >> 4));
        if index + 2 >= filtered.len() {
            break;
        }
        let b2 = filtered[index + 2];
        out.push((b1 << 4) | (b2 >> 2));
        if index + 3 >= filtered.len() {
            break;
        }
        let b3 = filtered[index + 3];
        out.push((b2 << 6) | b3);
        index += 4;
    }
    Some(out)
}

fn read_model_catalog() -> Result<Option<String>> {
    if let Some(explicit) = std::env::var_os("A3S_CURSOR_MODELS_FILE") {
        if explicit.is_empty() {
            return Ok(None);
        }
        let path = PathBuf::from(explicit);
        if !path.is_file() {
            return Ok(None);
        }
        let Some(raw) = read_bounded(&path, MAX_CATALOG_BYTES)
            .with_context(|| format!("could not read {}", path.display()))?
        else {
            tracing::warn!("cursor model list exceeded the read bound; skipping import");
            return Ok(None);
        };
        return Ok(Some(raw));
    }
    let Some(binary) = find_on_path("agent").or_else(|| find_on_path("cursor-agent")) else {
        return Ok(None);
    };
    let dir = std::env::temp_dir().join(format!(
        "a3s-cursor-models-{}-{}",
        std::process::id(),
        unix_now()
    ));
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let output = Command::new(&binary)
        .args(catalog_list_args())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(&dir)
        .output()
        .with_context(|| format!("could not run {}", binary.display()))?;
    let _ = std::fs::remove_dir_all(&dir);
    if !output.status.success() {
        tracing::warn!(status = %output.status, "cursor model list command failed");
        return Ok(None);
    }
    if output.stdout.len() > MAX_CATALOG_BYTES {
        tracing::warn!("cursor model list exceeded the read bound; skipping import");
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn parse_model_catalog(text: &str) -> Result<Vec<ModelConfig>> {
    let trimmed = text.trim().trim_start_matches('\u{feff}');
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value: Value =
            serde_json::from_str(trimmed).context("cursor model list is not valid JSON")?;
        return Ok(models_from_usable_json(&value));
    }
    Ok(models_from_cli_text(text))
}

fn models_from_usable_json(value: &Value) -> Vec<ModelConfig> {
    let entries = value
        .get("models")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for entry in entries {
        let model_id = entry
            .get("modelId")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let display_model_id = entry
            .get("displayModelId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let id = display_model_id.unwrap_or(model_id);
        if !valid_model_id(id) {
            continue;
        }
        let name = entry
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id);
        push_model(&mut models, id, name);
    }
    models
}

fn models_from_cli_text(text: &str) -> Vec<ModelConfig> {
    let mut models = Vec::new();
    for line in text.lines() {
        let line = strip_ansi(line).trim().to_string();
        if line.is_empty()
            || line.eq_ignore_ascii_case("Available models")
            || line.starts_with("Tip:")
        {
            continue;
        }
        let Some((id, rest)) = line.split_once(" - ") else {
            continue;
        };
        let id = id.trim();
        if !valid_model_id(id) {
            continue;
        }
        push_model(&mut models, id, &display_name(rest));
    }
    models
}

fn valid_model_id(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(char::is_whitespace)
}

fn is_placeholder_model(id: &str) -> bool {
    matches!(id.to_ascii_lowercase().as_str(), "auto" | "default")
}

fn display_name(rest: &str) -> String {
    let trimmed = rest.trim();
    for suffix in [" (current, default)", " (current)", " (default)"] {
        if let Some(name) = trimmed.strip_suffix(suffix) {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    if trimmed.is_empty() {
        "model".to_string()
    } else {
        trimmed.to_string()
    }
}

fn push_model(models: &mut Vec<ModelConfig>, id: &str, name: &str) {
    if models.iter().any(|model| model.id == id) {
        return;
    }
    models.push(model_config(id, name));
}

fn model_config(id: &str, name: &str) -> ModelConfig {
    ModelConfig {
        id: id.to_string(),
        name: name.to_string(),
        family: PROVIDER_NAME.to_string(),
        api_key: None,
        base_url: None,
        headers: Default::default(),
        session_id_header: None,
        attachment: false,
        reasoning: false,
        tool_call: true,
        temperature: true,
        release_date: None,
        modalities: ModelModalities::default(),
        cost: Default::default(),
        limit: Default::default(),
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn ghost_mode(config: &Value) -> bool {
    config
        .get("privacyCache")
        .and_then(|value| value.get("ghostMode"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn account_endpoint() -> String {
    let Ok(raw) = std::env::var("CURSOR_API_ENDPOINT") else {
        return DEFAULT_ENDPOINT.to_string();
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return DEFAULT_ENDPOINT.to_string();
    }
    match url::Url::parse(raw) {
        Ok(url)
            if matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            let mut endpoint = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
            if let Some(port) = url.port() {
                endpoint.push(':');
                endpoint.push_str(&port.to_string());
            }
            endpoint
        }
        _ => {
            tracing::warn!(
                "CURSOR_API_ENDPOINT is not an http(s) origin; using the default Cursor account endpoint"
            );
            DEFAULT_ENDPOINT.to_string()
        }
    }
}

fn client_version() -> String {
    if let Ok(value) = std::env::var("A3S_CURSOR_CLIENT_VERSION") {
        let value = value.trim();
        if !value.is_empty()
            && value.len() <= 128
            && !value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return value.to_string();
        }
    }
    if fixture_mode() {
        return "cli".to_string();
    }
    version_from_install().unwrap_or_else(|| "cli".to_string())
}

fn version_from_install() -> Option<String> {
    let path = find_on_path("agent").or_else(|| find_on_path("cursor-agent"))?;
    let canon = path.canonicalize().unwrap_or(path);
    let mut components = canon.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == "versions" {
            let version = components.next()?.as_os_str().to_string_lossy();
            if !version.is_empty() && !version.contains('/') && !version.contains('\\') {
                return Some(format!("cli-{version}"));
            }
        }
    }
    None
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > max_bytes as u64 {
        return Ok(None);
    }
    let raw = std::fs::read(path)?;
    if raw.len() > max_bytes {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    const BEARER: &str = "cursor-fixture-bearer-9f3a7c";
    const EMAIL: &str = "cursor-fixture-user@example.com";
    const OTHER_EMAIL: &str = "cursor-ide-other@example.com";

    struct EnvGuard {
        saved: Vec<(String, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&str, Option<&str>)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
                .collect();
            for (key, value) in pairs {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn isolated(test: impl FnOnce()) {
        let _lock = test_env_lock();
        let _guard = EnvGuard::set(&[
            ("A3S_DISABLE_CURSOR_ACCOUNT", None),
            ("A3S_PREFER_CURSOR", None),
            ("A3S_CURSOR_HOME", None),
            ("A3S_CURSOR_TOKEN_FILE", None),
            ("A3S_CURSOR_MODELS_FILE", None),
            ("A3S_CURSOR_STATE_DB", None),
            ("A3S_CURSOR_CLIENT_VERSION", None),
            ("CURSOR_API_ENDPOINT", None),
        ]);
        test();
    }

    #[derive(Clone)]
    struct LogSink(Arc<Mutex<Vec<u8>>>);

    struct LogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for LogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
        type Writer = LogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(self.0.clone())
        }
    }

    fn capture_logs(test: impl FnOnce()) -> String {
        let sink = LogSink(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, test);
        let bytes = sink
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn write_login(home: &Path, email: &str) {
        std::fs::write(
            home.join("cli-config.json"),
            format!(
                r#"{{
                    "authInfo": {{
                        "email": "{email}",
                        "authId": "fixture-auth-id",
                        "userId": "fixture-user",
                        "displayName": "Fixture"
                    }},
                    "privacyCache": {{ "ghostMode": false }}
                }}"#
            ),
        )
        .unwrap();
    }

    fn write_models(path: &Path) {
        std::fs::write(
            path,
            "Available models\n\n\u{1b}[32mauto\u{1b}[0m - Auto (current, default)\ncomposer-2.5 - Composer 2.5\ngpt-5.2 - GPT-5.2 (default)\nTip: press tab\nnot a row\n",
        )
        .unwrap();
    }

    fn install_sentinel(bin: &Path, marker: &Path) {
        std::fs::create_dir_all(bin).unwrap();
        let script = bin.join("agent");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' 'Available models' '' 'composer-2.5 - Composer 2.5'\n",
                marker.display()
            ),
        )
        .unwrap();
        let other = bin.join("cursor-agent");
        std::fs::write(
            &other,
            format!(
                "#!/bin/sh\nprintf '%s\\n' cursor-agent \"$@\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&script, mode.clone()).unwrap();
            std::fs::set_permissions(&other, mode).unwrap();
        }
    }

    #[test]
    fn fixture_login_merges_model_ids_without_logging_the_bearer() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let acl = root.path().join("config.acl");
            std::fs::write(
                &acl,
                "providers \"mock\" {\n  models \"m1\" { name = \"M1\" }\n}\n",
            )
            .unwrap();
            let bin = root.path().join("bin");
            let marker = root.path().join("spawned");
            install_sentinel(&bin, &marker);
            let mut paths = vec![bin.clone()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            let path = std::env::join_paths(&paths).unwrap();
            let path = path.to_string_lossy().into_owned();

            let logs = capture_logs(|| {
                let _env = EnvGuard::set(&[
                    ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                    ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                    ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
                    ("PATH", Some(&path)),
                ]);
                let mut config = a3s_code_core::CodeConfig::default();
                let merged = merge_into_config(&mut config).unwrap();
                assert!(merged);
                let provider = config
                    .providers
                    .iter()
                    .find(|provider| provider.name == PROVIDER_NAME)
                    .unwrap();
                let ids: Vec<_> = provider
                    .models
                    .iter()
                    .map(|model| model.id.as_str())
                    .collect();
                assert_eq!(ids, vec!["auto", "composer-2.5", "gpt-5.2"]);
                assert_eq!(
                    provider
                        .models
                        .iter()
                        .find(|model| model.id == "composer-2.5")
                        .map(|model| model.name.as_str()),
                    Some("Composer 2.5")
                );
                assert_eq!(provider.api_key.as_deref(), Some(BEARER));
                assert_eq!(provider.base_url.as_deref(), Some(DEFAULT_ENDPOINT));
                assert_eq!(
                    provider.headers.get("x-ghost-mode").map(String::as_str),
                    Some("false")
                );
                assert_eq!(config.default_model.as_deref(), Some("cursor/composer-2.5"));
                assert!(!marker.exists());
            });
            assert!(!logs.contains(BEARER), "bearer leaked into logs");
            assert!(!logs.contains(EMAIL), "email leaked into logs");
            let acl_text = std::fs::read_to_string(&acl).unwrap();
            assert!(!acl_text.contains(BEARER));
            assert!(!acl_text.contains(EMAIL));
            let login = std::fs::read_to_string(home.join("cli-config.json")).unwrap();
            assert!(!login.contains(BEARER));
        });
    }

    #[test]
    fn missing_login_and_disable_switch_merge_nothing() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let empty = root.path().join("empty-home");
            std::fs::create_dir_all(&empty).unwrap();
            let _env = EnvGuard::set(&[("A3S_CURSOR_HOME", Some(empty.to_str().unwrap()))]);
            let mut config = a3s_code_core::CodeConfig {
                default_model: Some("mock/m1".into()),
                ..Default::default()
            };
            assert!(!merge_into_config(&mut config).unwrap());
            assert!(config.providers.is_empty());
            assert_eq!(config.default_model.as_deref(), Some("mock/m1"));

            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let _env = EnvGuard::set(&[
                ("A3S_DISABLE_CURSOR_ACCOUNT", Some("1")),
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
            ]);
            assert!(!merge_into_config(&mut config).unwrap());
            assert!(config.providers.is_empty());
            assert_eq!(config.default_model.as_deref(), Some("mock/m1"));
        });
    }

    #[test]
    fn preset_default_model_is_kept_until_prefer_is_set() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let _env = EnvGuard::set(&[
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
            ]);
            let mut config = a3s_code_core::CodeConfig {
                default_model: Some("mock/m1".into()),
                ..Default::default()
            };
            assert!(merge_into_config(&mut config).unwrap());
            assert_eq!(config.default_model.as_deref(), Some("mock/m1"));

            let _prefer = EnvGuard::set(&[("A3S_PREFER_CURSOR", Some("yes"))]);
            assert!(merge_into_config(&mut config).unwrap());
            assert_eq!(config.default_model.as_deref(), Some("cursor/composer-2.5"));
            assert_eq!(
                config
                    .providers
                    .iter()
                    .filter(|provider| provider.name == PROVIDER_NAME)
                    .count(),
                1
            );
        });
    }

    #[test]
    fn usable_models_json_uses_display_model_id() {
        isolated(|| {
            let parsed = parse_model_catalog(
                r#"{"models":[
                    {"modelId":"default","displayModelId":"auto","displayName":"Auto"},
                    {"modelId":"composer-2.5","displayName":"Composer 2.5"}
                ]}"#,
            )
            .unwrap();
            let ids: Vec<_> = parsed.iter().map(|model| model.id.as_str()).collect();
            assert_eq!(ids, vec!["auto", "composer-2.5"]);
        });
    }

    #[test]
    fn auth_info_without_a_token_does_not_import() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("cli-config.json"), "{}").unwrap();
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let _env = EnvGuard::set(&[
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
            ]);
            let mut config = a3s_code_core::CodeConfig::default();
            assert!(!merge_into_config(&mut config).unwrap());
            assert!(config.providers.is_empty());
        });
    }

    #[test]
    fn expired_jwt_is_not_imported() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, "eyJhbGciOiJub25lIn0.eyJleHAiOjF9.eA").unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let _env = EnvGuard::set(&[
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
            ]);
            let mut config = a3s_code_core::CodeConfig::default();
            let logs = capture_logs(|| {
                assert!(!merge_into_config(&mut config).unwrap());
            });
            assert!(config.providers.is_empty());
            assert!(logs.contains("unusable"));
            assert!(!logs.contains("eyJhbGciOiJub25lIn0"));
        });
    }

    #[test]
    fn different_ide_account_keeps_the_cli_token() {
        isolated(|| {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let models = root.path().join("models.txt");
            write_models(&models);
            let db = root.path().join("state.vscdb");
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/cachedEmail', 'cursor-ide-other@example.com');
                 INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', 'ide-token-should-not-be-used');",
            )
            .unwrap();
            drop(conn);
            let _env = EnvGuard::set(&[
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("A3S_CURSOR_MODELS_FILE", Some(models.to_str().unwrap())),
                ("A3S_CURSOR_STATE_DB", Some(db.to_str().unwrap())),
            ]);
            let mut config = a3s_code_core::CodeConfig::default();
            let logs = capture_logs(|| {
                assert!(merge_into_config(&mut config).unwrap());
            });
            let provider = config
                .providers
                .iter()
                .find(|provider| provider.name == PROVIDER_NAME)
                .unwrap();
            assert_eq!(provider.api_key.as_deref(), Some(BEARER));
            assert!(logs.contains("different account"));
            assert!(logs.contains("IDE token was not used"));
            assert!(!logs.contains(BEARER));
            assert!(!logs.contains(EMAIL));
            assert!(!logs.contains(OTHER_EMAIL));
            assert!(!logs.contains("ide-token-should-not-be-used"));
        });
    }

    #[test]
    fn catalog_list_does_not_request_write_or_shell_access() {
        isolated(|| {
            assert_eq!(catalog_list_args(), ["models"]);
            for arg in catalog_list_args() {
                assert!(!matches!(
                    *arg,
                    "-p" | "--print" | "--force" | "--yolo" | "--approve-mcps"
                ));
            }
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("cursor-home");
            std::fs::create_dir_all(&home).unwrap();
            write_login(&home, EMAIL);
            let token = root.path().join("token");
            std::fs::write(&token, BEARER).unwrap();
            let bin = root.path().join("bin");
            let marker = root.path().join("argv");
            install_sentinel(&bin, &marker);
            let workspace = root.path().join("workspace");
            std::fs::create_dir_all(&workspace).unwrap();
            let mut paths = vec![bin.clone()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            let path = std::env::join_paths(&paths).unwrap();
            let path = path.to_string_lossy().into_owned();
            let _env = EnvGuard::set(&[
                ("A3S_CURSOR_HOME", Some(home.to_str().unwrap())),
                ("A3S_CURSOR_TOKEN_FILE", Some(token.to_str().unwrap())),
                ("PATH", Some(&path)),
            ]);
            let mut config = a3s_code_core::CodeConfig::default();
            assert!(merge_into_config(&mut config).unwrap());
            let argv = std::fs::read_to_string(&marker).unwrap();
            assert_eq!(argv.trim(), "models");
            assert!(std::fs::read_dir(&workspace).unwrap().next().is_none());
            let ids: Vec<_> = config
                .providers
                .iter()
                .find(|provider| provider.name == PROVIDER_NAME)
                .unwrap()
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect();
            assert_eq!(ids, vec!["composer-2.5"]);
        });
    }

    #[test]
    #[ignore = "reads this machine's Cursor login and prints model ids only"]
    fn live_cursor_catalog_matches_agent_models() {
        isolated(|| {
            // Launch from the process workspace so ancestor ACL layers are
            // the ones `a3s code` reads. A temp directory has none of them.
            let workspace = std::env::current_dir().expect("cwd");
            let launch =
                crate::config::load_launch_config(workspace, None).expect("shipped launch");
            println!("launch ok {}", launch.model_id);
            let config = launch.config;
            let provider = config
                .providers
                .iter()
                .find(|provider| provider.name == PROVIDER_NAME)
                .expect("cursor provider missing from launch config");
            for model in &provider.models {
                println!("cursor-model-id {}", model.id);
            }
            let output = std::process::Command::new("agent")
                .args(catalog_list_args())
                .env("NO_COLOR", "1")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .expect("agent models");
            assert!(
                output.status.success(),
                "agent models exited {:?}",
                output.status.code()
            );
            let listed = parse_model_catalog(&String::from_utf8_lossy(&output.stdout))
                .expect("agent models parse");
            assert!(!listed.is_empty(), "agent models returned no ids");
            for model in &listed {
                assert!(
                    provider.models.iter().any(|merged| merged.id == model.id),
                    "CLI model {} missing from merged catalog",
                    model.id
                );
            }
            assert!(provider
                .api_key
                .as_deref()
                .is_some_and(|key| !key.is_empty()));
        });
    }

    #[test]
    #[ignore = "one short completion on the logged-in Cursor account"]
    fn live_cursor_completion_returns_assistant_text() {
        let _lock = test_env_lock();
        let _guard = EnvGuard::set(&[
            ("A3S_DISABLE_CURSOR_ACCOUNT", None),
            ("A3S_PREFER_CURSOR", None),
            ("A3S_CURSOR_HOME", None),
            ("A3S_CURSOR_TOKEN_FILE", None),
            ("A3S_CURSOR_MODELS_FILE", None),
            ("A3S_CURSOR_STATE_DB", None),
            ("A3S_CURSOR_CLIENT_VERSION", None),
            ("CURSOR_API_ENDPOINT", None),
        ]);
        let workspace = tempfile::tempdir().unwrap();
        let sentinel = workspace.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"unchanged").unwrap();
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(workspace.path()).expect("enter workspace");
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let _restore = Restore(prior);

        let mut config = a3s_code_core::CodeConfig::default();
        assert!(merge_into_config(&mut config).expect("merge"));
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.name == PROVIDER_NAME)
            .expect("cursor provider");
        let model_id = if provider.models.iter().any(|model| model.id == "gpt-5.2") {
            "gpt-5.2".to_string()
        } else {
            provider
                .models
                .iter()
                .find(|model| !is_placeholder_model(&model.id))
                .map(|model| model.id.clone())
                .expect("non-auto model")
        };
        println!("live-model {model_id}");
        let llm = config
            .llm_config(PROVIDER_NAME, &model_id)
            .expect("session llm config");
        let client = a3s_code_core::llm::create_client_with_config(llm);
        // Keep this environment-mutating fixture serialized while a private
        // runtime drives the request, without holding a std lock in an async
        // test future that can suspend on the shared executor.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        let response = runtime
            .block_on(client.complete(
                &[a3s_code_core::Message::user(
                    "Reply with the single word pong.",
                )],
                None,
                &[] as &[a3s_code_core::llm::ToolDefinition],
            ))
            .expect("live completion");
        let text = response.text();
        assert!(!text.trim().is_empty(), "empty assistant text");
        assert!(!text.contains("eyJ"), "assistant text looked like a token");
        println!("live-assistant {text}");
        let mut names: Vec<_> = std::fs::read_dir(workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, vec![std::ffi::OsString::from("sentinel.txt")]);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
    }
}
