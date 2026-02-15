//! A3S Code Agent Binary
//!
//! Entry point for the coding agent that runs as a gRPC + REST service.
//! Workspace and LLM configuration are provided per-session by clients
//! via CreateSession / ConfigureSession RPCs or REST API.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use a3s_code::config::CodeConfig;
use a3s_code::rest::{self, AppState};
use a3s_code::telemetry_init::{self, TelemetryConfig};

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

    /// REST API listen address
    #[arg(long, env = "REST_ADDR", default_value = "0.0.0.0:4089")]
    rest_addr: String,

    /// Disable REST API server
    #[arg(long, env = "NO_REST")]
    no_rest: bool,

    /// Bearer token for REST API authentication (optional)
    #[arg(long, env = "A3S_API_TOKEN")]
    api_token: Option<String>,

    /// OpenTelemetry OTLP endpoint (e.g., http://localhost:4317)
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,

    /// Output logs in JSON format
    #[arg(long, env = "A3S_LOG_FORMAT")]
    json_log: bool,
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
        json_log: args.json_log,
        ..TelemetryConfig::default()
    };
    telemetry_init::init_telemetry(&telemetry_config);

    tracing::info!("Starting A3S Code Agent v{}", env!("CARGO_PKG_VERSION"));

    // Load config file if provided (for skills, agents, storage settings)
    let config_path = args.config.as_deref();
    let config = match config_path {
        Some(path) if path.exists() => CodeConfig::from_file(path)?,
        _ => CodeConfig::default(),
    };

    // Start REST API server (if enabled)
    if !args.no_rest {
        let rest_state = AppState {
            agents: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(config.clone())),
            api_token: args.api_token.clone(),
        };
        let rest_addr = args.rest_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = rest::start_rest_server(rest_state, &rest_addr).await {
                tracing::error!("REST server error: {e}");
            }
        });
    }

    // Start gRPC service (blocking)
    let result = a3s_code::service::start_server_with_config(
        config,
        &args.listen_addr,
        config_path,
    )
    .await;

    telemetry_init::shutdown_telemetry();
    result
}
