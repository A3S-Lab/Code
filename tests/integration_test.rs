//! Integration tests for A3S Code
//!
//! These tests use the real configuration file from .a3s/config.json
//! and test the actual functionality of the A3S Code agent.

use a3s_box_code::config::CodeConfig;
use std::path::PathBuf;

/// Helper to get the config path
fn get_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".a3s")
        .join("config.json")
}

#[test]
fn test_load_config_from_a3s_dir() {
    let config_path = get_config_path();

    if !config_path.exists() {
        eprintln!("Config file not found at {:?}, skipping test", config_path);
        return;
    }

    let config =
        CodeConfig::from_file(&config_path).expect("Failed to load config from .a3s/config.json");

    // Verify config structure
    assert!(
        config.default_provider.is_some(),
        "Default provider should be set"
    );
    assert!(
        config.default_model.is_some(),
        "Default model should be set"
    );
    assert!(
        !config.providers.is_empty(),
        "Providers should not be empty"
    );

    println!("✓ Loaded config from .a3s/config.json");
    println!("  Default provider: {:?}", config.default_provider);
    println!("  Default model: {:?}", config.default_model);
    println!("  Providers: {}", config.providers.len());
}

#[test]
fn test_config_providers() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    // Test provider lookup
    let default_provider_name = config.default_provider.as_ref().unwrap();
    let provider = config.find_provider(default_provider_name);
    assert!(provider.is_some(), "Default provider should exist");

    let provider = provider.unwrap();
    println!("✓ Found provider: {}", provider.name);
    println!("  Models: {}", provider.models.len());

    // Test model lookup
    let default_model_id = config.default_model.as_ref().unwrap();
    let model = provider.find_model(default_model_id);
    assert!(model.is_some(), "Default model should exist in provider");

    let model = model.unwrap();
    println!("✓ Found model: {} ({})", model.name, model.id);
}

#[test]
fn test_config_model_details() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    // Get default model config
    let (provider, model) = config
        .default_model_config()
        .expect("Default model config should be available");

    println!("✓ Default model configuration:");
    println!("  Provider: {}", provider.name);
    println!("  Model ID: {}", model.id);
    println!("  Model Name: {}", model.name);
    println!("  Family: {}", model.family);
    println!("  Tool Call: {}", model.tool_call);
    println!("  Reasoning: {}", model.reasoning);
    println!("  Attachment: {}", model.attachment);

    // Verify model capabilities
    assert!(model.tool_call, "Default model should support tool calls");
}

#[test]
fn test_config_llm_config() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    // Get LLM config for default provider/model
    let llm_config = config
        .default_llm_config()
        .expect("LLM config should be available");

    println!("✓ LLM configuration:");
    println!("  Provider: {}", llm_config.provider);
    println!("  Model: {}", llm_config.model);
    println!(
        "  API Key: {}",
        if llm_config.api_key.is_empty() {
            "(not set)"
        } else {
            "(set)"
        }
    );
    println!("  Base URL: {:?}", llm_config.base_url);

    // Verify API key is set
    assert!(!llm_config.api_key.is_empty(), "API key should be set");
}

#[test]
fn test_list_all_models() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    let models = config.list_models();
    println!("✓ Available models ({}):", models.len());

    for (provider, model) in &models {
        println!(
            "  - {}/{}: {} (tool_call: {})",
            provider.name, model.id, model.name, model.tool_call
        );
    }

    assert!(!models.is_empty(), "Should have at least one model");
}

#[test]
fn test_config_model_cost() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();
    let (_, model) = config.default_model_config().unwrap();

    println!("✓ Model cost (per million tokens):");
    println!("  Input: ${}", model.cost.input);
    println!("  Output: ${}", model.cost.output);
    println!("  Cache Read: ${}", model.cost.cache_read);
    println!("  Cache Write: ${}", model.cost.cache_write);

    println!("✓ Model limits:");
    println!("  Context: {} tokens", model.limit.context);
    println!("  Output: {} tokens", model.limit.output);
}

#[test]
fn test_config_model_modalities() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();
    let (_, model) = config.default_model_config().unwrap();

    println!("✓ Model modalities:");
    println!("  Input: {:?}", model.modalities.input);
    println!("  Output: {:?}", model.modalities.output);
}

#[test]
fn test_config_alternate_provider() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    // Test finding alternate provider (openai)
    if let Some(provider) = config.find_provider("openai") {
        println!("✓ Found alternate provider: {}", provider.name);
        println!("  Models: {}", provider.models.len());

        for model in &provider.models {
            println!("  - {}: {}", model.id, model.name);
            if let Some(base_url) = &model.base_url {
                println!("    Base URL: {}", base_url);
            }
        }
    } else {
        println!("  No alternate provider 'openai' configured");
    }
}

#[test]
fn test_config_llm_config_for_specific_model() {
    let config_path = get_config_path();

    if !config_path.exists() {
        return;
    }

    let config = CodeConfig::from_file(&config_path).unwrap();

    // Test getting LLM config for a specific provider/model
    if let Some(llm_config) = config.llm_config("anthropic", "claude-opus-4-5-20251101") {
        println!("✓ LLM config for claude-opus-4-5:");
        println!("  Provider: {}", llm_config.provider);
        println!("  Model: {}", llm_config.model);
        println!(
            "  API Key: {}",
            if llm_config.api_key.is_empty() {
                "(not set)"
            } else {
                "(set)"
            }
        );
    }
}
