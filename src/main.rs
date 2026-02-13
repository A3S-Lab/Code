//! A3S Code Agent Binary
//!
//! Entry point for the coding agent that runs as a gRPC service.
//! Workspace and LLM configuration are provided per-session by clients
//! via CreateSession / ConfigureSession RPCs.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use a3s_code::config::CodeConfig;
use a3s_code::telemetry::{self, TelemetryConfig};

/// A3S Code Agent - AI coding assistant with tool execution capabilities
#[derive(Parser, Debug)]
#[command(name = "a3s-code")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    serve_args: ServeArgs,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Update a3s-code to the latest version
    Update,
}

#[derive(clap::Args, Debug)]
struct ServeArgs {
    /// Path to config.json file (for skills, agents, storage settings)
    #[arg(short = 'c', long, env = "A3S_CONFIG")]
    config: Option<PathBuf>,

    /// gRPC server listen address
    #[arg(short = 'l', long, env = "LISTEN_ADDR", default_value = "0.0.0.0:4088")]
    listen_addr: String,

    /// OpenTelemetry OTLP endpoint (e.g., http://localhost:4317)
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle update subcommand early
    if matches!(cli.command, Some(Commands::Update)) {
        return a3s_updater::run_update(&a3s_updater::UpdateConfig {
            binary_name: "a3s-code",
            crate_name: "a3s-code",
            current_version: env!("CARGO_PKG_VERSION"),
            github_owner: "A3S-Lab",
            github_repo: "Code",
        })
        .await;
    }

    let args = cli.serve_args;

    // Initialize telemetry
    let telemetry_config = TelemetryConfig {
        otlp_endpoint: args.otlp_endpoint.clone(),
        ..TelemetryConfig::default()
    };
    telemetry::init_telemetry(&telemetry_config);

    tracing::info!("Starting A3S Code Agent v{}", env!("CARGO_PKG_VERSION"));

    // Load config file if provided (for skills, agents, storage settings)
    let config_path = args.config.as_deref();
    let config = match config_path {
        Some(path) if path.exists() => CodeConfig::from_file(path)?,
        _ => CodeConfig::default(),
    };

    // Start gRPC service
    let result = a3s_code::service::start_server_with_config(
        config,
        &args.listen_addr,
        config_path,
    )
    .await;

    telemetry::shutdown_telemetry();
    result
}
