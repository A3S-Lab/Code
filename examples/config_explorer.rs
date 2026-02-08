//! Config Explorer Example
//!
//! Demonstrates loading and exploring the A3S Code configuration
//! from the real `.a3s/config.json` file.
//!
//! Run with:
//!   cargo run --example config_explorer
//!
//! Or with a custom config path:
//!   A3S_CONFIG=path/to/config.json cargo run --example config_explorer

use a3s_box_code::config::CodeConfig;
use std::path::PathBuf;

fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║         A3S Code - Config Explorer               ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // --- 1. Load config from .a3s/config.json ---
    let config = load_config();

    // --- 2. Display provider information ---
    println!("📋 Configuration Summary");
    println!("  Default Provider: {:?}", config.default_provider);
    println!("  Default Model:    {:?}", config.default_model);
    println!("  Storage Backend:  {:?}", config.storage_backend);
    println!("  Providers:        {}", config.providers.len());
    println!();

    // --- 3. List all models across providers ---
    let models = config.list_models();
    println!("🤖 Available Models ({}):", models.len());
    for (provider, model) in &models {
        println!("  ┌─ {}/{}", provider.name, model.id);
        println!("  │  Name:       {}", model.name);
        println!("  │  Family:     {}", model.family);
        println!("  │  Tool Call:  {}", model.tool_call);
        println!("  │  Reasoning:  {}", model.reasoning);
        println!("  │  Attachment: {}", model.attachment);
        println!("  │  Context:    {} tokens", model.limit.context);
        println!("  │  Output:     {} tokens", model.limit.output);
        if model.cost.input > 0.0 {
            println!(
                "  │  Cost:       ${:.2} input / ${:.2} output (per 1M tokens)",
                model.cost.input, model.cost.output
            );
        }
        if let Some(ref url) = model.base_url {
            println!("  │  Base URL:   {}", url);
        }
        println!("  └─");
    }
    println!();

    // --- 4. Get default LLM config ---
    match config.default_llm_config() {
        Some(llm_config) => {
            println!("🔑 Default LLM Config:");
            println!("  Provider:  {}", llm_config.provider);
            println!("  Model:     {}", llm_config.model);
            println!(
                "  API Key:   {}...{}",
                &llm_config.api_key[..8.min(llm_config.api_key.len())],
                if llm_config.api_key.len() > 12 {
                    &llm_config.api_key[llm_config.api_key.len() - 4..]
                } else {
                    ""
                }
            );
            if let Some(ref url) = llm_config.base_url {
                println!("  Base URL:  {}", url);
            }
        }
        None => {
            println!("⚠️  No default LLM config available (missing provider/model/API key)");
        }
    }
    println!();

    // --- 5. Test specific model lookup ---
    println!("🔍 Model Lookup Tests:");
    let test_lookups = [
        ("anthropic", "claude-sonnet-4-20250514"),
        ("anthropic", "claude-opus-4-5-20251101"),
        ("openai", "kimi-k2.5"),
        ("openai", "gpt-4o"),
    ];

    for (provider_name, model_id) in test_lookups {
        match config.llm_config(provider_name, model_id) {
            Some(cfg) => println!(
                "  ✅ {}/{} → found (key: {}...)",
                provider_name,
                model_id,
                &cfg.api_key[..8.min(cfg.api_key.len())]
            ),
            None => println!("  ❌ {}/{} → not found", provider_name, model_id),
        }
    }
    println!();

    // --- 6. Config merge demonstration ---
    println!("🔀 Config Merge Demo:");
    let mut base_config = CodeConfig::new()
        .add_skill_dir("~/.a3s/skills")
        .add_agent_dir("~/.a3s/agents");

    println!(
        "  Before merge: {} skill dirs, {} agent dirs",
        base_config.skill_dirs.len(),
        base_config.agent_dirs.len()
    );

    let overlay = CodeConfig::new()
        .add_skill_dir("/project/.a3s/skills")
        .with_watch(true);

    base_config.merge(overlay);

    println!(
        "  After merge:  {} skill dirs, {} agent dirs, watch={}",
        base_config.skill_dirs.len(),
        base_config.agent_dirs.len(),
        base_config.watch_enabled
    );

    println!("\n✅ Config exploration complete!");
}

/// Load configuration from the project's .a3s directory or environment
fn load_config() -> CodeConfig {
    // Try environment variable first
    if let Ok(config_path) = std::env::var("A3S_CONFIG") {
        println!("📂 Loading config from A3S_CONFIG={}\n", config_path);
        return CodeConfig::from_file(&PathBuf::from(config_path))
            .expect("Failed to load config from A3S_CONFIG");
    }

    // Try the project's .a3s/config.json
    let project_config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".a3s")
        .join("config.json");

    if project_config.exists() {
        println!("📂 Loading config from {}\n", project_config.display());
        return CodeConfig::from_file(&project_config)
            .expect("Failed to load config from .a3s/config.json");
    }

    // Fall back to default locations
    println!("📂 Loading config from default locations\n");
    CodeConfig::load_default()
}
