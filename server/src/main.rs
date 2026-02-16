//! A3S Code Agent Binary
//!
//! Entry point for the A3S Code agent. Provides CLI for configuration
//! and self-update. The agent is used as a library via native SDKs
//! (Python, Node.js) or directly via the Rust API.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use a3s_code::config::CodeConfig;
use a3s_code::telemetry_init::{self, TelemetryConfig};

/// A3S Code Agent - AI coding assistant with tool execution capabilities
#[derive(Parser, Debug)]
#[command(name = "a3s-code")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to config file
    #[arg(short = 'c', long, env = "A3S_CONFIG")]
    config: Option<PathBuf>,

    /// OpenTelemetry OTLP endpoint (e.g., http://localhost:4317)
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,

    /// Output logs in JSON format
    #[arg(long, env = "A3S_LOG_FORMAT")]
    json_log: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Update a3s-code to the latest version
    Update,
    /// Print version and config info
    Info,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle subcommands
    match cli.command {
        Some(Commands::Update) => {
            return a3s_updater::run_update(&a3s_updater::UpdateConfig {
                binary_name: "a3s-code",
                crate_name: "a3s-code",
                current_version: env!("CARGO_PKG_VERSION"),
                github_owner: "A3S-Lab",
                github_repo: "Code",
            })
            .await;
        }
        Some(Commands::Info) => {
            println!("a3s-code v{}", env!("CARGO_PKG_VERSION"));
            if let Some(ref path) = cli.config {
                println!("config: {}", path.display());
            }
            return Ok(());
        }
        None => {}
    }

    // Initialize telemetry
    let telemetry_config = TelemetryConfig {
        otlp_endpoint: cli.otlp_endpoint.clone(),
        json_log: cli.json_log,
        ..TelemetryConfig::default()
    };
    telemetry_init::init_telemetry(&telemetry_config);

    tracing::info!("A3S Code Agent v{}", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = match cli.config.as_deref() {
        Some(path) if path.exists() => CodeConfig::from_file(path)?,
        _ => CodeConfig::default(),
    };

    // Build agent from config
    let agent = a3s_code::Agent::builder()
        .with_config(config)
        .build()
        .await?;

    tracing::info!("Agent ready. Use native SDKs (Python/Node.js) or Rust API to interact.");

    // Keep process alive for SDK connections
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");
    drop(agent);

    telemetry_init::shutdown_telemetry();
    Ok(())
}
