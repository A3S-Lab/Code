//! A3S Code Agent Binary
//!
//! Entry point for the coding agent that runs inside the guest VM.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use a3s_box_code::config::CodeConfig;

/// A3S Code Agent - AI coding assistant with tool execution capabilities
#[derive(Parser, Debug)]
#[command(name = "a3s-code")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to config directory containing config.json
    #[arg(short = 'd', long, env = "A3S_CONFIG_DIR")]
    config_dir: Option<PathBuf>,

    /// Path to config.json file
    #[arg(short = 'c', long, env = "A3S_CONFIG")]
    config: Option<PathBuf>,

    /// gRPC server listen address
    #[arg(short = 'l', long, env = "LISTEN_ADDR", default_value = "0.0.0.0:4088")]
    listen_addr: String,

    /// Workspace directory
    #[arg(short = 'w', long, env = "A3S_WORKSPACE")]
    workspace: Option<PathBuf>,

    /// Session storage backend (memory, file)
    #[arg(long, env = "A3S_STORAGE_BACKEND")]
    storage_backend: Option<String>,

    /// Sessions directory (for file backend)
    #[arg(long, env = "A3S_SESSIONS_DIR")]
    sessions_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("Starting A3S Code Agent");
    tracing::info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // Parse CLI arguments
    let args = Args::parse();

    // Load configuration
    let config = load_config(&args)?;

    // Log configuration status
    if config.has_providers() {
        tracing::info!(
            "Loaded {} provider(s) from config",
            config.providers.len()
        );
        if let Some(ref provider) = config.default_provider {
            if let Some(ref model) = config.default_model {
                tracing::info!("Default model: {}/{}", provider, model);
            }
        }
    } else {
        tracing::info!("No providers configured, clients must provide via ConfigureSession RPC");
    }

    if config.has_directories() {
        tracing::info!(
            "Skill directories: {:?}, Agent directories: {:?}",
            config.skill_dirs,
            config.agent_dirs
        );
    }

    // Determine workspace
    let workspace = args
        .workspace
        .map(|p| p.to_string_lossy().to_string())
        .or_else(|| std::env::var("WORKSPACE").ok())
        .unwrap_or_else(|| "/a3s/workspace".to_string());

    // Start gRPC service
    a3s_box_code::service::start_server_with_config(config, &workspace, &args.listen_addr).await?;

    Ok(())
}

/// Load configuration from CLI arguments or default locations
fn load_config(args: &Args) -> Result<CodeConfig> {
    // Priority: --config > --config-dir > default locations
    if let Some(ref config_path) = args.config {
        tracing::info!("Loading config from: {}", config_path.display());
        let mut config = CodeConfig::from_file(config_path)?;

        // Override with CLI arguments if provided
        if let Some(ref backend) = args.storage_backend {
            config.storage_backend = parse_storage_backend(backend)?;
        }
        if let Some(ref dir) = args.sessions_dir {
            config.sessions_dir = Some(dir.clone());
        }

        return Ok(config);
    }

    if let Some(ref config_dir) = args.config_dir {
        tracing::info!("Loading config from directory: {}", config_dir.display());
        let mut config = CodeConfig::from_dir(config_dir)?;

        // Override with CLI arguments if provided
        if let Some(ref backend) = args.storage_backend {
            config.storage_backend = parse_storage_backend(backend)?;
        }
        if let Some(ref dir) = args.sessions_dir {
            config.sessions_dir = Some(dir.clone());
        }

        return Ok(config);
    }

    // Try default locations
    let config = CodeConfig::load_default();

    // Merge with environment variables
    let env_config = CodeConfig::from_env();
    let mut final_config = config;
    final_config.merge(env_config);

    // Override with CLI arguments if provided
    if let Some(ref backend) = args.storage_backend {
        final_config.storage_backend = parse_storage_backend(backend)?;
    }
    if let Some(ref dir) = args.sessions_dir {
        final_config.sessions_dir = Some(dir.clone());
    }

    Ok(final_config)
}

/// Parse storage backend string to enum
fn parse_storage_backend(s: &str) -> Result<a3s_box_code::config::StorageBackend> {
    use a3s_box_code::config::StorageBackend;

    match s.to_lowercase().as_str() {
        "memory" => Ok(StorageBackend::Memory),
        "file" => Ok(StorageBackend::File),
        _ => Err(anyhow::anyhow!(
            "Invalid storage backend '{}'. Valid options: memory, file",
            s
        )),
    }
}
