//! First-party API-key environment primitives.
//!
//! Only the env-key checks the auth subsystem itself needs live here. The ACP
//! `auth_methods` list-building surface (`build_auth_methods`,
//! `AuthMethodsBuildInputs`, `should_advertise_a3s_api_key`, ...) stays in
//! `a3s_code_shell::agent::auth_method`, which depends on shell's `ModelEntry`.

use std::sync::RwLock;

/// Env var that, when set, advertises `a3s.api_key` as a viable auth method.
///
/// Kept as a constant so test code and the production check stay in sync.
pub const A3S_API_KEY_ENV_VAR: &str = "A3S_API_KEY";

/// Legacy env var name.
/// Checked as a fallback when `A3S_API_KEY` is not set, so existing deployments that use the old name keep working.
pub const LEGACY_A3S_API_KEY_ENV_VAR: &str = "GROK_CODE_A3S_API_KEY";

/// Runtime-loaded keys live here instead of the process env: `set_var` races C `getenv` on other threads (DNS, libgit2).
static RUNTIME_API_KEY: RwLock<Option<String>> = RwLock::new(None);

pub fn set_runtime_a3s_api_key(key: &str) {
    *RUNTIME_API_KEY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(key.to_owned());
}

/// Falls back to the process env; an inherited key can only be removed by the shell that exported it.
pub fn clear_runtime_a3s_api_key() {
    *RUNTIME_API_KEY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// `std::env::var` that also sees the runtime key when `name` is `A3S_API_KEY`.
pub fn read_env_var(name: &str) -> Result<String, std::env::VarError> {
    if name == A3S_API_KEY_ENV_VAR
        && let Some(key) = RUNTIME_API_KEY
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    {
        return Ok(key);
    }
    std::env::var(name)
}

/// Read the API key from the runtime override or the environment.
///
/// Checks `A3S_API_KEY` first, then falls back to the legacy `GROK_CODE_A3S_API_KEY` for backward compatibility.
pub fn read_a3s_api_key_env() -> Result<String, std::env::VarError> {
    read_env_var(A3S_API_KEY_ENV_VAR).or_else(|_| std::env::var(LEGACY_A3S_API_KEY_ENV_VAR))
}

/// Returns `true` if either `A3S_API_KEY` or `GROK_CODE_A3S_API_KEY` is set.
pub fn has_a3s_api_key_env() -> bool {
    read_a3s_api_key_env().is_ok()
}
