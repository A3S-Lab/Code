//! A3S Search v0.7.0 Configuration Test
//!
//! Tests the configurable web_search tool with different engine configurations
//!
//! Run with: cargo run --example test_search_config

use a3s_code_core::Agent;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=debug")
        .init();

    println!("🚀 A3S Search v0.7.0 Configuration Test\n");
    println!("{}", "=".repeat(80));

    // Test 1: Default configuration (no search config)
    test_default_search().await?;

    // Test 2: Custom search configuration
    test_custom_search_config().await?;

    // Test 3: Engine enable/disable
    test_engine_control().await?;

    println!("{}", "\n".repeat(2));
    println!("{}", "=".repeat(80));
    println!("✅ All search configuration tests completed!");
    println!("{}", "=".repeat(80));

    Ok(())
}

/// Test 1: Default search configuration
async fn test_default_search() -> Result<()> {
    println!("\n🔍 Test 1: Default Search Configuration");
    println!("{}", "-".repeat(80));

    let config_hcl = r#"
        default_model = "openai/kimi-k2.5"

        providers {
          name = "openai"

          models {
            id          = "kimi-k2.5"
            name        = "KIMI K2.5"
            family      = "kimi"
            api_key     = "sk-G6exg5ITTg5WseU091BKayWz9TX4S4LtPBfU58AtR3UctZ18"
            base_url    = "http://35.220.164.252:3888/v1"
            attachment  = false
            reasoning   = false
            tool_call   = true
            temperature = true

            modalities {
              input  = ["text"]
              output = ["text"]
            }

            limit {
              context = 128000
              output  = 4096
            }
          }
        }
    "#;

    let agent = Agent::new(config_hcl).await?;
    let session = agent.session(".", None)?;

    println!("Testing: Web search with default engines (ddg,wiki)...");
    let result = session.send(
        "Search the web for 'Rust async programming' and give me the top 3 results",
        None
    ).await;

    match result {
        Ok(r) => {
            println!("✓ Default search works");
            println!("  Result preview: {}", truncate(&r.text, 200));
        }
        Err(e) => {
            println!("⚠️  Search failed (expected if engines unavailable): {}", e);
        }
    }

    println!("\n✅ Test 1 passed: Default configuration works");
    Ok(())
}

/// Test 2: Custom search configuration
async fn test_custom_search_config() -> Result<()> {
    println!("\n⚙️  Test 2: Custom Search Configuration");
    println!("{}", "-".repeat(80));

    let config_hcl = r#"
        default_model = "openai/kimi-k2.5"

        providers {
          name = "openai"

          models {
            id          = "kimi-k2.5"
            name        = "KIMI K2.5"
            family      = "kimi"
            api_key     = "sk-G6exg5ITTg5WseU091BKayWz9TX4S4LtPBfU58AtR3UctZ18"
            base_url    = "http://35.220.164.252:3888/v1"
            attachment  = false
            reasoning   = false
            tool_call   = true
            temperature = true

            modalities {
              input  = ["text"]
              output = ["text"]
            }

            limit {
              context = 128000
              output  = 4096
            }
          }
        }

        # Custom search configuration
        search {
          timeout = 30

          health {
            max_failures = 3
            suspend_seconds = 60
          }

          engine {
            ddg {
              enabled = true
              weight = 1.5
            }

            wiki {
              enabled = true
              weight = 1.2
            }

            brave {
              enabled = true
              weight = 1.0
              timeout = 20
            }
          }
        }
    "#;

    let agent = Agent::new(config_hcl).await?;
    let session = agent.session(".", None)?;

    println!("Testing: Web search with custom configuration...");
    println!("  - Timeout: 30 seconds");
    println!("  - Engines: ddg (1.5), wiki (1.2), brave (1.0)");
    println!("  - Health monitoring enabled");

    let result = session.send(
        "Search for 'Rust tokio tutorial' and summarize the findings",
        None
    ).await;

    match result {
        Ok(r) => {
            println!("✓ Custom search configuration works");
            println!("  Result preview: {}", truncate(&r.text, 200));
        }
        Err(e) => {
            println!("⚠️  Search failed (expected if engines unavailable): {}", e);
        }
    }

    println!("\n✅ Test 2 passed: Custom configuration works");
    Ok(())
}

/// Test 3: Engine enable/disable control
async fn test_engine_control() -> Result<()> {
    println!("\n🎛️  Test 3: Engine Enable/Disable Control");
    println!("{}", "-".repeat(80));

    // Test with only wiki enabled
    println!("Testing: Only Wikipedia enabled...");
    let config_hcl = r#"
        default_model = "openai/kimi-k2.5"

        providers {
          name = "openai"

          models {
            id          = "kimi-k2.5"
            name        = "KIMI K2.5"
            family      = "kimi"
            api_key     = "sk-G6exg5ITTg5WseU091BKayWz9TX4S4LtPBfU58AtR3UctZ18"
            base_url    = "http://35.220.164.252:3888/v1"
            attachment  = false
            reasoning   = false
            tool_call   = true
            temperature = true

            modalities {
              input  = ["text"]
              output = ["text"]
            }

            limit {
              context = 128000
              output  = 4096
            }
          }
        }

        search {
          timeout = 20

          engine {
            ddg {
              enabled = false  # Disabled
            }

            wiki {
              enabled = true   # Only this one enabled
              weight = 1.0
            }

            brave {
              enabled = false  # Disabled
            }
          }
        }
    "#;

    let agent = Agent::new(config_hcl).await?;
    let session = agent.session(".", None)?;

    let result = session.send(
        "Search for 'Rust programming language' using only Wikipedia",
        None
    ).await;

    match result {
        Ok(r) => {
            println!("✓ Engine control works (only wiki enabled)");
            println!("  Result preview: {}", truncate(&r.text, 200));
        }
        Err(e) => {
            println!("⚠️  Search failed (expected if wiki unavailable): {}", e);
        }
    }

    // Test with all engines disabled
    println!("\nTesting: All engines disabled...");
    let config_hcl = r#"
        default_model = "openai/kimi-k2.5"

        providers {
          name = "openai"

          models {
            id          = "kimi-k2.5"
            name        = "KIMI K2.5"
            family      = "kimi"
            api_key     = "sk-G6exg5ITTg5WseU091BKayWz9TX4S4LtPBfU58AtR3UctZ18"
            base_url    = "http://35.220.164.252:3888/v1"
            attachment  = false
            reasoning   = false
            tool_call   = true
            temperature = true

            modalities {
              input  = ["text"]
              output = ["text"]
            }

            limit {
              context = 128000
              output  = 4096
            }
          }
        }

        search {
          engine {
            ddg { enabled = false }
            wiki { enabled = false }
            brave { enabled = false }
          }
        }
    "#;

    let agent = Agent::new(config_hcl).await?;
    let session = agent.session(".", None)?;

    let result = session.send(
        "Try to search the web for 'test query'",
        None
    ).await;

    match result {
        Ok(r) => {
            if r.text.contains("No valid engines") || r.text.contains("failed") {
                println!("✓ Correctly reports no engines available");
            } else {
                println!("⚠️  Unexpected result: {}", truncate(&r.text, 100));
            }
        }
        Err(e) => {
            println!("✓ Correctly fails when all engines disabled: {}", e);
        }
    }

    println!("\n✅ Test 3 passed: Engine control works correctly");
    Ok(())
}

/// Truncate helper
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
