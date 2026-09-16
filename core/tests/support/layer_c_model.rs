//! Layer C live E2E model binding for the monorepo Flash default.
//!
//! Pins `boyue/deepseek-v4-flash` when that model exists under
//! `providers "boyue"`, matching monorepo `.a3s/config.acl` `default_model`.
//! `A3S_TEST_MODEL` overrides the pin when the named route exists in the ACL.

use std::path::PathBuf;

use a3s_code_core::config::CodeConfig;

/// Goal-required Layer C default (`provider/model` form).
pub const REQUIRED_DEFAULT_MODEL: &str = "boyue/deepseek-v4-flash";
const REQUIRED_PROVIDER: &str = "boyue";
const REQUIRED_MODEL_ID: &str = "deepseek-v4-flash";

/// Second Flash route used for live session model-switch coverage.
pub const ALTERNATE_FLASH_MODEL: &str = "boyue/bailian/deepseek-v4.1-flash";
const ALTERNATE_MODEL_ID: &str = "bailian/deepseek-v4.1-flash";

pub fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

fn resolve_provider_model(full: &str) -> Option<(&str, &str)> {
    let (provider, model) = full.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

fn provider_has_model(config: &CodeConfig, provider: &str, model_id: &str) -> bool {
    config
        .find_provider(provider)
        .map(|p| p.models.iter().any(|model| model.id == model_id))
        .unwrap_or(false)
}

/// Load ACL and pin the Layer C Flash model when the provider declares it.
pub fn load_pinned_layer_c_config() -> CodeConfig {
    let path = repo_config_path();
    let mut config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let provider = config.find_provider(REQUIRED_PROVIDER).unwrap_or_else(|| {
        panic!(
            "{} must declare providers \"{REQUIRED_PROVIDER}\"",
            path.display()
        )
    });
    assert!(
        provider
            .models
            .iter()
            .any(|model| model.id == REQUIRED_MODEL_ID),
        "{} must declare {REQUIRED_PROVIDER} models \"{REQUIRED_MODEL_ID}\"",
        path.display()
    );

    let pin = std::env::var("A3S_TEST_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|full| {
            // Common typo: bailian Flash is `…/deepseek-v4.1-flash` in ACL.
            let resolved = if full == "boyue/bailian/deepseek-v4-flash" {
                eprintln!(
                    "A3S_TEST_MODEL={full} is not declared; using {ALTERNATE_FLASH_MODEL}"
                );
                ALTERNATE_FLASH_MODEL.to_string()
            } else {
                full
            };
            let (provider_id, model_id) = resolve_provider_model(&resolved)?;
            if provider_has_model(&config, provider_id, model_id) {
                Some(resolved)
            } else {
                eprintln!(
                    "A3S_TEST_MODEL={resolved} is not declared in {}; falling back to {REQUIRED_DEFAULT_MODEL}",
                    path.display()
                );
                None
            }
        })
        .unwrap_or_else(|| REQUIRED_DEFAULT_MODEL.to_string());

    if config.default_model.as_deref() != Some(pin.as_str()) {
        eprintln!(
            "pinning default_model to {pin} (config had {:?})",
            config.default_model
        );
    }
    config.default_model = Some(pin.clone());
    eprintln!("using default_model={pin}");
    config
}

/// True when `model` is either declared Layer C Flash peer.
pub fn is_layer_c_flash_model(model: Option<&str>) -> bool {
    matches!(
        model,
        Some(REQUIRED_DEFAULT_MODEL) | Some(ALTERNATE_FLASH_MODEL)
    )
}

/// Live suites accept either Flash peer (default or bailian), including the
/// common typo `boyue/bailian/deepseek-v4-flash` after loader remapping.
pub fn assert_pinned_layer_c_flash(config: &CodeConfig, suite: &str) {
    let pin = config.default_model.as_deref();
    assert!(
        is_layer_c_flash_model(pin),
        "{suite} must pin a Layer C Flash route ({REQUIRED_DEFAULT_MODEL} or {ALTERNATE_FLASH_MODEL}); got {pin:?}"
    );
}

/// Resolve a declared Layer C model after applying `A3S_TEST_MODEL` remapping.
///
/// Prefer this over reading `A3S_TEST_MODEL` raw — the loader maps the common
/// bailian typo `boyue/bailian/deepseek-v4-flash` onto the declared route.
pub fn pinned_layer_c_model(config: &CodeConfig) -> String {
    assert_pinned_layer_c_flash(config, "Layer C live suite");
    config
        .default_model
        .clone()
        .expect("Layer C pin must set default_model")
}

/// Confirm a second Flash route exists for model-switch E2E.
///
/// Picks a declared Flash route distinct from `primary` so the switch test
/// still works when the Layer C pin is either Flash id.
pub fn alternate_flash_model(config: &CodeConfig, primary: &str) -> &'static str {
    assert!(
        provider_has_model(config, REQUIRED_PROVIDER, REQUIRED_MODEL_ID),
        "config must declare {REQUIRED_PROVIDER} models \"{REQUIRED_MODEL_ID}\" for model-switch E2E"
    );
    assert!(
        provider_has_model(config, REQUIRED_PROVIDER, ALTERNATE_MODEL_ID),
        "config must declare {REQUIRED_PROVIDER} models \"{ALTERNATE_MODEL_ID}\" for model-switch E2E"
    );
    if primary == ALTERNATE_FLASH_MODEL {
        REQUIRED_DEFAULT_MODEL
    } else if primary == REQUIRED_DEFAULT_MODEL {
        ALTERNATE_FLASH_MODEL
    } else if primary.starts_with("boyue/") {
        // Pinned to some other boyue route — prefer the non-default Flash peer.
        if primary.contains("bailian") {
            REQUIRED_DEFAULT_MODEL
        } else {
            ALTERNATE_FLASH_MODEL
        }
    } else {
        ALTERNATE_FLASH_MODEL
    }
}
