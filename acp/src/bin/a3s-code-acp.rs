//! Stdio ACP agent binary for the forked a3s-build pager.

use std::path::PathBuf;
use std::rc::Rc;

use a3s_code_acp::unattended::{Unattended, DEFAULT_EXECUTION_BUDGET_MS};
use a3s_code_acp::{load_launch_config_with_admission, A3sCodeAgent, PluginSelectionHost};
use agent_client_protocol as acp;
use anyhow::{Context, Result};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

struct Args {
    cwd: PathBuf,
    config: Option<PathBuf>,
    model: Option<String>,
    plugin_store: Option<PathBuf>,
    studio_documents: bool,
    unattended: Option<Unattended>,
    workspace_mcp: Option<WorkspaceMcpCommand>,
}

/// Review or trust the repository-declared MCP servers, then exit.
#[derive(Debug, PartialEq, Eq)]
enum WorkspaceMcpCommand {
    List,
    Trust(String),
}

fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(mut args: impl Iterator<Item = String>) -> Result<Args> {
    let mut cwd = std::env::current_dir().context("current_dir")?;
    let mut config: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut plugin_store = None;
    let mut studio_documents = false;
    let mut unattended = false;
    let mut max_execution_ms = None;
    let mut result_file = None;
    let mut reasoning_effort = None;
    let mut engineering_policy = None;
    let mut goal_continuation = false;
    let mut workspace_mcp = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--unattended" => unattended = true,
            "--list-workspace-mcp" => workspace_mcp = Some(WorkspaceMcpCommand::List),
            "--trust-workspace-mcp" => {
                workspace_mcp = Some(WorkspaceMcpCommand::Trust(
                    args.next()
                        .context("--trust-workspace-mcp requires an id prefix or `all`")?,
                ));
            }
            "--goal-continuation" => goal_continuation = true,
            "--max-execution-ms" => {
                let value = args.next().context("--max-execution-ms requires a value")?;
                max_execution_ms = Some(
                    value
                        .parse::<u64>()
                        .context("--max-execution-ms must be an integer")?,
                );
            }
            "--reasoning-effort" => {
                reasoning_effort =
                    Some(args.next().context("--reasoning-effort requires a level")?);
            }
            "--engineering-policy" => {
                engineering_policy = Some(
                    args.next()
                        .context("--engineering-policy requires a policy name")?,
                );
            }
            "--result-file" => {
                result_file = Some(PathBuf::from(
                    args.next().context("--result-file requires a path")?,
                ));
            }
            // A launcher that still passes this does not exit. New Studio
            // sessions ask in `_meta.a3sStudioDocuments` instead.
            "--studio-documents" => studio_documents = true,
            "--plugin-store" => {
                plugin_store = Some(PathBuf::from(
                    args.next().context("--plugin-store requires a path")?,
                ));
            }
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
            "--version" | "-V" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: a3s-code-acp [--cwd DIR] [--config FILE] [--model provider/model] [--plugin-store DIR]\n\
                     \x20      a3s-code-acp --list-workspace-mcp | --trust-workspace-mcp <ID|all> [--cwd DIR]\n\
                     \x20      a3s-code-acp --unattended [--max-execution-ms MS] [--reasoning-effort LEVEL] [--engineering-policy engineering-v1] [--goal-continuation] [--result-file FILE] [--cwd DIR] [--config FILE] [--model provider/model]\n\
                     Speaks Agent Client Protocol on stdio over a3s-code-core {}.",
                    env!("CARGO_PKG_VERSION")
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
    let unattended = if unattended {
        if plugin_store.is_some() || studio_documents {
            anyhow::bail!("--unattended does not combine with Studio host flags");
        }
        let profile = Unattended::new(
            max_execution_ms.unwrap_or(DEFAULT_EXECUTION_BUDGET_MS),
            result_file,
        )?;
        let profile = match reasoning_effort {
            Some(level) => profile.with_reasoning_effort(&level)?,
            None => profile,
        };
        let profile = match engineering_policy {
            Some(name) => profile.with_engineering_policy(&name)?,
            None => profile,
        };
        Some(if goal_continuation {
            profile.with_goal_continuation()
        } else {
            profile
        })
    } else {
        if max_execution_ms.is_some()
            || result_file.is_some()
            || reasoning_effort.is_some()
            || engineering_policy.is_some()
            || goal_continuation
        {
            anyhow::bail!(
                "--max-execution-ms, --reasoning-effort, --engineering-policy, --goal-continuation and --result-file require --unattended"
            );
        }
        None
    };
    Ok(Args {
        cwd,
        config,
        model,
        plugin_store,
        studio_documents,
        unattended,
        workspace_mcp,
    })
}

fn run_workspace_mcp_command(args: &Args, command: &WorkspaceMcpCommand) -> Result<()> {
    let config = args.config.as_deref();
    let declarations = match command {
        WorkspaceMcpCommand::List => a3s_code_acp::workspace_mcp_declarations(&args.cwd, config)?,
        WorkspaceMcpCommand::Trust(selector) => {
            a3s_code_acp::trust_workspace_mcp(&args.cwd, config, selector)?
        }
    };
    if declarations.is_empty() {
        println!("No workspace MCP servers to review.");
    }
    for declaration in declarations {
        let state = match (command, declaration.trusted) {
            (WorkspaceMcpCommand::Trust(_), _) => "trusted",
            (_, true) => "trusted",
            (_, false) => "pending",
        };
        println!(
            "{}  {:8} {}  {}",
            declaration.short_id(),
            state,
            declaration.name,
            declaration.summary
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "--version" || arg == "-V" {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    }
    // ACP requests stay on this LocalSet via spawn_local. The fact worker is
    // tokio::spawn on the pool. A tool poll that does not yield must not
    // freeze the task that reads session/cancel. Do not spawn_local from a
    // pool thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async { run().await })
}

async fn run() -> Result<()> {
    use tracing_subscriber::layer::{Layer as _, SubscriberExt};
    use tracing_subscriber::util::SubscriberInitExt;
    // stderr stays at the quiet default (warn); the daily file layer runs at
    // info so hangs and failures leave an on-disk trail the terminal's
    // scrollback cannot keep. A failed file open degrades to stderr-only.
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        );
    let registry = tracing_subscriber::registry().with(stderr_layer);
    if a3s_code_acp::log_file::file_logging_enabled() {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(a3s_code_acp::log_file::DailyLogWriter::new())
                    .with_filter(
                        tracing_subscriber::EnvFilter::try_from_env("A3S_CODE_LOG_FILE")
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    ),
            )
            .init();
    } else {
        registry.init();
    }

    let args = parse_args()?;
    if let Some(command) = &args.workspace_mcp {
        return run_workspace_mcp_command(&args, command);
    }
    if let Some(model) = args.model {
        std::env::set_var("A3S_DEFAULT_MODEL", model);
    }
    let (mut launch, admission) =
        load_launch_config_with_admission(args.cwd, args.config).context("load A3S ACL")?;
    if args.plugin_store.is_some() || args.studio_documents {
        launch.workspace =
            std::path::absolute(&launch.workspace).context("bind native workspace")?;
    }
    // A goal launch fills `goal_control` only when the ACL declares none.
    if let (Some(unattended), None) = (&args.unattended, &launch.config.goal_control) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("host clock")?
            .as_millis();
        launch.config.goal_control =
            unattended.goal_control(u64::try_from(now).context("host clock")?)?;
    }
    let workspace = launch.workspace.clone();
    let mut agent = A3sCodeAgent::new(launch).with_workspace_admission(admission);
    if let Some(unattended) = args.unattended {
        let root = a3s_code_acp::unattended::install_state_root()?;
        tracing::info!(state_root = %root.display(), "unattended launch");
        agent = agent.with_unattended(unattended);
    }
    if let Some(root) = args.plugin_store {
        let store =
            a3s_use_reconcile::ReconcileStore::open(root).context("open native plugin store")?;
        agent = agent.with_plugin_selection(
            PluginSelectionHost::current(store, "studio-installed-plugins", &workspace).await?,
        );
    }
    if args.studio_documents {
        agent = agent.with_studio_documents();
    }
    let agent = Rc::new(agent);

    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();
    let (conn, io_task) = acp::AgentSideConnection::new(agent.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });
    agent.set_client(Rc::new(conn));
    let result = io_task.await;
    agent.stop_document_bridge();
    result.context("ACP stdio I/O")?;
    Ok(())
}

#[cfg(test)]
#[path = "a3s-code-acp/tests.rs"]
mod tests;
