//! A3S Code Agent Binary
//!
//! Entry point for the coding agent that runs inside the guest VM.

use anyhow::Result;

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

    // Start gRPC service
    a3s_box_code::service::start_server().await?;

    Ok(())
}
