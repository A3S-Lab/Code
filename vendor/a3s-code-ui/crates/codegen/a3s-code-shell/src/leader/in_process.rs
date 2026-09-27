//! A real agent behind a leader server, in this process rather than a child.

use std::rc::Rc;
use std::sync::Arc;

use a3s_acp_lib::{
    AcpAgentGatewayReceiver as GatewayReceiver, AcpAgentGatewaySender as GatewaySender,
    LineBufferedRead,
};
use agent_client_protocol as acp;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, simplex};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use crate::agent::config::Config as AgentConfig;
use crate::agent::mvp_agent::MvpAgent;

const SIMPLEX_BUF: usize = 8 * 1024 * 1024;

/// An in-process agent: the tasks that pump its I/O, and the handle that keeps it allocated.
///
/// `MvpAgent` spawns background tasks on the ambient `LocalSet` that hold raw `LocalRef` self-pointers, so the agent
/// must outlive every task on that `LocalSet`. Aborting `tasks` ends the ACP connection (which owns its own clone of
/// the agent) but leaves those background tasks queued; a caller that lets the agent die while they are still queued
/// frees the memory they keep reading, and their teardown writes through the dangling pointer.
/// Hold `keepalive` past the `LocalSet`: `drop(local_set)` first, then `drop(keepalive)`.
pub struct InProcessAgent {
    /// The ACP connection, the request pump and the response pump. Abort and await them to end the agent's I/O.
    pub tasks: Vec<JoinHandle<()>>,
    /// Drop only after the `LocalSet` the agent was spawned on.
    pub keepalive: Rc<MvpAgent>,
}

/// Spawns an agent on the current `LocalSet`, reading requests from `to_agent` and writing responses to `from_agent`.
/// Resolves once the agent is built, with the task handles so a caller can end its I/O and the keepalive the caller
/// must hold past the `LocalSet`. Panics if the ambient configuration cannot build one.
pub async fn spawn_agent(
    mut to_agent: UnboundedReceiver<String>,
    from_agent: UnboundedSender<String>,
) -> InProcessAgent {
    let (agent_in_read, mut agent_in_write) = simplex(SIMPLEX_BUF);
    let (agent_out_read, agent_out_write) = simplex(SIMPLEX_BUF);
    let (keepalive_tx, keepalive_rx) = oneshot::channel();

    // Built inside the task so the agent's bootstrap state lives on the task's heap-boxed future, not on the caller's
    // stack (a debug-build `MvpAgent` boot would otherwise deepen every test that awaits this)
    let connection = tokio::task::spawn_local(async move {
        let mut config = AgentConfig::default();
        let auth_manager = Arc::new(config.create_auth_manager());
        // This runs on a current-thread `LocalSet`, where the sync bootstrap in `MvpAgent::new`
        // cannot drive the settings load itself. Resolve it on the async runtime first so
        // bootstrap observes a finished wait rather than falling open to bundled defaults.
        let boot = crate::agent::init::resolve_boot_startup_settings(
            &mut config,
            &tokio_util::sync::CancellationToken::new(),
            true,
            auth_manager.current(),
        )
        .await
        .ok();
        let (gateway_tx, gateway_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent = Rc::new(
            MvpAgent::new(
                GatewaySender::new(gateway_tx),
                &config,
                auth_manager,
                None,
                boot,
            )
            .expect("valid agent config"),
        );
        // A caller that stopped waiting has no way to hold the agent past the `LocalSet`, so do not start one
        if keepalive_tx.send(Rc::clone(&agent)).is_err() {
            return;
        }
        let incoming = LineBufferedRead::spawn_local(agent_in_read.compat());
        let (conn, handle_io) =
            acp::AgentSideConnection::new(agent, agent_out_write.compat_write(), incoming, |fut| {
                tokio::task::spawn_local(fut);
            });
        tokio::task::spawn_local(
            GatewayReceiver::new(gateway_rx, conn)
                .with_on_meta(a3s_code_otel::span_from_meta_traceparent)
                .run(),
        );
        let _ = handle_io.await;
    });

    let requests = tokio::task::spawn_local(async move {
        while let Some(msg) = to_agent.recv().await {
            if agent_in_write.write_all(msg.as_bytes()).await.is_err()
                || agent_in_write.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
    });

    let responses = tokio::task::spawn_local(async move {
        let mut reader = BufReader::new(agent_out_read);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let msg = line.trim_end_matches(['\r', '\n']).to_string();
                    if !msg.is_empty() {
                        let _ = from_agent.send(msg);
                    }
                }
            }
        }
    });

    let keepalive = keepalive_rx
        .await
        .expect("in-process agent task ended before it built an agent");
    InProcessAgent {
        tasks: vec![connection, requests, responses],
        keepalive,
    }
}
