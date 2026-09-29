//! Stdio ACP agent binary for the forked a3s-build pager.

use std::path::PathBuf;
use std::rc::Rc;

use a3s_code_acp::{load_launch_config, A3sCodeAgent};
use agent_client_protocol as acp;
use anyhow::{Context, Result};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

fn parse_args() -> Result<(PathBuf, Option<PathBuf>, Option<String>)> {
    let mut cwd = std::env::current_dir().context("current_dir")?;
    let mut config: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--cwd requires a path"))?;
                cwd = PathBuf::from(value);
            }
            "--config" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
                config = Some(PathBuf::from(value));
            }
            "--model" | "-m" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--model requires a value"))?;
                model = Some(value);
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: a3s-code-acp [--cwd DIR] [--config FILE] [--model provider/model]\n\
                     Speaks Agent Client Protocol on stdio over a3s-code-core 9.0.0."
                );
                std::process::exit(0);
            }
            other => {
                return Err(anyhow::anyhow!("unknown argument: {other}"));
            }
        }
    }
    // The pager forwards the ACL as `A3S_CONFIG`. `--config` still wins.
    if config.is_none() {
        if let Some(value) = std::env::var_os("A3S_CONFIG").filter(|value| !value.is_empty()) {
            config = Some(PathBuf::from(value));
        }
    }
    Ok((cwd, config, model))
}

fn main() -> Result<()> {
    // Agent-side ACP uses LocalSet / spawn_local.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async { run().await })
}

async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let (cwd, config, model) = parse_args()?;
    if let Some(model) = model {
        std::env::set_var("A3S_DEFAULT_MODEL", model);
    }
    let launch = load_launch_config(cwd, config).context("load A3S ACL")?;
    let agent = Rc::new(A3sCodeAgent::new(launch));

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();
    let (conn, io_task) = acp::AgentSideConnection::new(agent.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });
    agent.set_client(Rc::new(conn));
    io_task.await.context("ACP stdio I/O")?;
    Ok(())
}
