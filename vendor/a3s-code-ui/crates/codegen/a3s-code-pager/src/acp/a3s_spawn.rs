//! Spawn the A3S Code ACP agent as a stdio subprocess and bridge it into
//! the pager's typed ACP channels (same pattern as [`super::leader_bridge`]).

use std::process::Stdio;
use std::thread;

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use a3s_acp_lib::{AcpGatewayReceiver, AcpGatewaySender, LineBufferedRead, acp_channels};
use a3s_code_shell::util::grok_home::grok_home;
use agent_client_protocol as acp;

use super::spawn::{SpawnedAgent, boot_auth_manager};
use a3s_code_shell::agent::config::Config as AgentConfig;

const MAX_BUF: usize = 8 * 1024 * 1024;

/// Spawn `A3S_ACP_AGENT_BIN` (or fail) and bridge its stdio into an [`SpawnedAgent`].
pub async fn spawn_a3s_acp(
    agent_config: &AgentConfig,
    cancel: &CancellationToken,
    cwd: Option<&std::path::Path>,
    model: Option<&str>,
) -> Result<SpawnedAgent> {
    let bin = std::env::var("A3S_ACP_AGENT_BIN")
        .map_err(|_| anyhow!("A3S_ACP_AGENT_BIN is not set; cannot spawn a3s-code-acp"))?;
    let auth_manager = boot_auth_manager(&grok_home(), agent_config);

    let agent_cancel = cancel.child_token();
    let (client_channel, agent_channel) = acp_channels();

    let cwd = cwd
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let model = model.map(str::to_owned);
    let bridge_cancel = agent_cancel.clone();

    let thread_handle = thread::Builder::new()
        .name("a3s-acp-bridge".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || -> Result<()> {
            let mut builder = tokio::runtime::Builder::new_current_thread();
            let rt = a3s_tty_utils::runtime::apply_blocking_pool(builder.enable_all()).build()?;
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let mut command = Command::new(&bin);
                command
                    .arg("--cwd")
                    .arg(&cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true);
                if let Some(model) = model.as_deref() {
                    command.arg("--model").arg(model);
                }
                let mut child = command
                    .spawn()
                    .with_context(|| format!("failed to spawn a3s ACP agent at {bin}"))?;
                let child_stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| anyhow!("a3s ACP agent stdin missing"))?;
                let child_stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| anyhow!("a3s ACP agent stdout missing"))?;

                let (incoming_read, mut incoming_write) = tokio::io::simplex(MAX_BUF);
                let (outgoing_read, outgoing_write) = tokio::io::simplex(MAX_BUF);

                // Child stdout → ClientSideConnection incoming
                let cancel_r = bridge_cancel.clone();
                let reader_task = tokio::task::spawn_local(async move {
                    let mut reader = BufReader::new(child_stdout);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        tokio::select! {
                            biased;
                            _ = cancel_r.cancelled() => break,
                            result = reader.read_line(&mut line) => {
                                match result {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        if incoming_write.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                });

                // ClientSideConnection outgoing → child stdin
                let cancel_w = bridge_cancel.clone();
                let writer_task = tokio::task::spawn_local(async move {
                    let mut reader = BufReader::new(outgoing_read);
                    let mut stdin = child_stdin;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        tokio::select! {
                            biased;
                            _ = cancel_w.cancelled() => break,
                            result = reader.read_line(&mut line) => {
                                match result {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        if stdin.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                });

                let gw_tx = AcpGatewaySender::new(agent_channel.tx).with_tracing(true);
                let incoming = LineBufferedRead::spawn_local(incoming_read.compat());
                let (conn, handle_io) = acp::ClientSideConnection::new(
                    gw_tx,
                    outgoing_write.compat_write(),
                    incoming,
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );
                let gw_rx = AcpGatewayReceiver::new(agent_channel.rx, conn).with_tracing(true);
                tokio::task::spawn_local(handle_io);
                tokio::task::spawn_local(gw_rx.run());
                tokio::task::yield_now().await;

                bridge_cancel.cancelled().await;
                reader_task.abort();
                writer_task.abort();
                let _ = child.kill().await;
                Ok(())
            })
        })
        .context("failed to start a3s ACP bridge thread")?;

    Ok(SpawnedAgent {
        thread_handle,
        channel: client_channel,
        cancel: agent_cancel,
        auth_manager,
    })
}

/// True when the pager should use the A3S ACP subprocess instead of A3S Code shell.
pub fn a3s_acp_enabled() -> bool {
    std::env::var_os("A3S_ACP_AGENT_BIN").is_some()
}
