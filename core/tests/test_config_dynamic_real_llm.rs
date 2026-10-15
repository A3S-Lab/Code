//! Live config retrieval, layering, and dynamic reload against real traffic.
//!
//! Two product contracts:
//!
//! 1. **Layered overlay drives real traffic** — a user-layer provider with a
//!    dead endpoint is overlaid by a workspace layer carrying the live
//!    credentials; the merged document must serve a real model turn through
//!    the overlaid provider (before the overlay fix, the user layer's
//!    credentials won and the turn died on the dead URL).
//! 2. **Dynamic write → reload chain** — the section-preserving config
//!    editor rewrites `default_model` on disk (comments, `env()` references,
//!    and unrelated blocks survive); the reparsed file drives a real model
//!    turn. This is the `a3s model use` dynamic-config path end to end.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_config_dynamic_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::time::{Duration, Instant};

use a3s_code_core::config::{CodeConfig, ConfigSection};
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

const TURN_TIMEOUT: Duration = Duration::from_secs(420);

/// The pinned live provider's resolved endpoint and credential, used to
/// template the workspace overlay layer.
struct LiveEndpoint {
    provider: String,
    base_url: String,
    api_key: String,
}

fn live_endpoint() -> LiveEndpoint {
    let config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    let (provider, _) = model.split_once('/').expect("pinned provider/model");
    let provider_config = config
        .providers
        .iter()
        .find(|candidate| candidate.name == provider)
        .expect("pinned provider config");
    LiveEndpoint {
        provider: provider.to_owned(),
        base_url: provider_config
            .base_url
            .clone()
            .expect("pinned provider baseUrl"),
        api_key: provider_config
            .api_key
            .clone()
            .expect("pinned provider apiKey"),
    }
}

fn acl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn run_turn(agent: &Agent, model: &str, session_id: &str) -> String {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id(session_id)
        .with_model(model)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session from merged config");
    let (mut events, worker) = session
        .stream("Reply with exactly the single word: READY", None)
        .await
        .expect("start turn");
    let final_text = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await.expect("stream open") {
                AgentEvent::End { text, .. } => break text,
                AgentEvent::Error { message } => panic!("turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("turn timed out");
    worker.await.expect("worker joins");
    session.close().await;
    final_text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn layered_overlay_serves_real_traffic_through_the_workspace_layer() {
    let started = Instant::now();
    let endpoint = live_endpoint();
    let pinned = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&pinned);
    let (_, model_id) = model.split_once('/').expect("pinned model id");

    // The USER layer declares the provider with a dead endpoint and stale
    // credentials; the WORKSPACE layer overlays the live endpoint. Layers
    // join exactly as the ACP discovery joins them.
    let user_layer = format!(
        r#"
default_model = "{provider}/dead-fallback"

providers "{provider}" {{
  apiKey = "stale-user-layer-key"
  baseUrl = "http://dead.user.layer.invalid/v1/"
  models "dead-fallback" {{
    name = "dead-fallback"
    family = ""
    toolCall = true
  }}
}}
"#,
        provider = acl_escape(&endpoint.provider),
    );
    let workspace_layer = format!(
        r#"
default_model = "{model}"

providers "{provider}" {{
  apiKey = "{api_key}"
  baseUrl = "{base_url}"
  models "{model_id}" {{
    name = "{model_id}"
    family = ""
    toolCall = true
    temperature = true
  }}
}}
"#,
        model = acl_escape(&model),
        provider = acl_escape(&endpoint.provider),
        api_key = acl_escape(&endpoint.api_key),
        base_url = acl_escape(&endpoint.base_url),
        model_id = acl_escape(model_id),
    );

    let mut merged =
        CodeConfig::from_acl(&format!("{user_layer}\n{workspace_layer}")).expect("merged layers");
    merged.overlay_duplicate_providers();

    // Resolution consistency: one provider, workspace overlay won.
    assert_eq!(merged.providers.len(), 1, "layers merge to one provider");
    let resolved = merged
        .default_llm_config()
        .expect("default model resolves through the overlay");
    assert_eq!(resolved.model, model_id);
    assert_eq!(resolved.api_key.expose(), endpoint.api_key);
    assert_eq!(
        resolved.base_url.as_deref(),
        Some(endpoint.base_url.as_str())
    );

    // The live discriminator: real traffic must run on the overlaid
    // endpoint (the user layer's dead URL would fail the turn).
    let agent = Agent::from_config(merged)
        .await
        .expect("agent from overlay");
    let answer = run_turn(&agent, &model, "config-layer-overlay").await;
    assert!(
        answer.to_uppercase().contains("READY"),
        "the overlaid provider must serve real traffic: {answer:?}"
    );
    println!(
        "config-layer-overlay-real-llm provider={} model={} total_ms={} result=pass",
        endpoint.provider,
        model_id,
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn section_edit_persists_reloads_and_serves_real_traffic() {
    let started = Instant::now();
    let endpoint = live_endpoint();
    let pinned = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&pinned);
    let (_, model_id) = model.split_once('/').expect("pinned model id");

    // A realistic on-disk config: comments, an env() credential indirection
    // seeded through the environment, and an unrelated unknown-tolerated
    // block (storage) that the edit must preserve.
    let api_key_var = "A3S_CONFIG_DYNAMIC_TEST_KEY";
    std::env::set_var(api_key_var, &endpoint.api_key);
    let original = format!(
        r#"// Workspace configuration managed by hand.
// The default model below is switched by `a3s model use`.

storage_backend = "file"

default_model = "{provider}/placeholder"

providers "{provider}" {{
  apiKey = env("{api_key_var}")
  baseUrl = "{base_url}"
  models "{model_id}" {{
    name = "{model_id}"
    family = ""
    toolCall = true
    temperature = true
  }}
}}
"#,
        provider = acl_escape(&endpoint.provider),
        api_key_var = api_key_var,
        base_url = acl_escape(&endpoint.base_url),
        model_id = acl_escape(model_id),
    );

    let config_dir = tempfile::tempdir().expect("config dir");
    let config_path = config_dir.path().join("config.acl");
    std::fs::write(&config_path, &original).expect("write config");

    // The dynamic write path: load, switch the default model section,
    // persist (section-preserving), reload from disk.
    let mut config = CodeConfig::from_file(&config_path).expect("parse the on-disk config");
    config.default_model = Some(model.clone());
    let updated = a3s_code_core::config::rewrite_acl_sections(
        &original,
        &config,
        &[ConfigSection::DefaultModel],
    )
    .expect("rewrite the default_model section");
    std::fs::write(&config_path, &updated).expect("persist the edit");

    // Preservation contract: comments, the env() indirection, and the
    // storage block survive the rewrite verbatim.
    assert!(
        updated.contains("// Workspace configuration managed by hand."),
        "comments must survive: {updated:?}"
    );
    assert!(
        updated.contains(&format!("env(\"{api_key_var}\")")),
        "env() indirection must survive: {updated:?}"
    );
    assert!(
        updated.contains("storage_backend = \"file\""),
        "unrelated sections must survive: {updated:?}"
    );
    assert!(
        updated.contains(&format!("default_model = \"{}\"", acl_escape(&model))),
        "the new default must be present: {updated:?}"
    );

    // Reload from disk and serve real traffic through the reloaded config.
    let reloaded = CodeConfig::from_file(&config_path).expect("reload after edit");
    assert_eq!(reloaded.default_model.as_deref(), Some(model.as_str()));
    let llm = reloaded
        .default_llm_config()
        .expect("env() credential resolves after reload");
    assert_eq!(llm.api_key.expose(), endpoint.api_key);

    let agent = Agent::from_config(reloaded)
        .await
        .expect("agent from reload");
    let answer = run_turn(&agent, &model, "config-dynamic-reload").await;
    assert!(
        answer.to_uppercase().contains("READY"),
        "the reloaded config must serve real traffic: {answer:?}"
    );
    std::env::remove_var(api_key_var);
    println!(
        "config-dynamic-reload-real-llm provider={} model={} edit_bytes={} total_ms={} result=pass",
        endpoint.provider,
        model_id,
        updated.len(),
        started.elapsed().as_millis()
    );
}
