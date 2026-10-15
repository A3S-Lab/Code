#![cfg(test)]

//! FAULT-LIVE1 — fault-injection live matrix on the fact spine.
//!
//! World-class harnesses survive provider failure without hanging,
//! poisoning the session, or losing the workspace: this gate drives the
//! REAL session path (fact spine) against injected faults and then
//! proves full recovery on the REAL pinned model.
//!
//! Legs:
//! A. Provider down (connection refused): the turn fails classified and
//!    bounded — never a hang.
//! B. Stalled socket (accepts, never writes): the per-attempt timeout
//!    ends response observation and requests cooperative cancellation;
//!    original cleanup waits for the same actual model child to join.
//! C. The same workspace on the REAL pinned model answers normally:
//!    the fault legs poisoned nothing (process, workspace, fact store).
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   rustup run 1.97.1 cargo test -p a3s-code-core --test test_fault_recovery_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::{Agent, CodeError, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

const FAULT_TIMEOUT_MS: u64 = 2_000;
/// Generous wall bound: the transport client may retry the fault, and
/// each attempt re-pays the timeout. The pin is "bounded", not "fast".
const FAULT_WALL_LIMIT: Duration = Duration::from_secs(90);

fn fault_config(endpoint: &str) -> a3s_code_core::CodeConfig {
    let mut config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned config carries default_model");
    let provider_name = model.split_once('/').expect("provider/model").0.to_string();
    let provider = config
        .providers
        .iter_mut()
        .find(|provider| provider.name == provider_name)
        .expect("pinned provider present");
    provider.base_url = Some(format!("http://{endpoint}"));
    config
}

fn fault_opts(session_id: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_planning(false)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_llm_api_timeout(FAULT_TIMEOUT_MS)
}

/// A port nothing listens on: bind :0, read the port, drop the listener.
fn dead_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

/// The fixture owns accepted sockets until its task is dropped. Aborting this
/// test server on Drop does not abort or certify the original model child.
struct StallingServer {
    port: u16,
    accepted: Arc<tokio::sync::Semaphore>,
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

async fn stalling_server() -> StallingServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stall server");
    let port = listener.local_addr().expect("addr").port();
    let accepted = Arc::new(tokio::sync::Semaphore::new(0));
    let observed = accepted.clone();
    let task = tokio::spawn(async move {
        let mut sockets = Vec::new();
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            sockets.push(socket);
            observed.add_permits(1);
            // Keep every accepted connection open without producing bytes.
        }
    });
    StallingServer {
        port,
        accepted,
        _task: tokio_util::task::AbortOnDropHandle::new(task),
    }
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn fault_legs_fail_bounded_and_the_real_model_recovers() {
    let workspace = tempfile::tempdir().expect("workspace");

    // Leg A — provider down (connection refused).
    let agent = Agent::from_config(fault_config(&format!("127.0.0.1:{}", dead_port())))
        .await
        .expect("fault agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(fault_opts("fault-dead-provider")),
        )
        .await
        .expect("fault session");
    let started = Instant::now();
    let dead = session.send("reply with anything", None).await;
    let dead_elapsed = started.elapsed();
    println!("FAULTLIVE leg A (connection refused) elapsed: {dead_elapsed:.2?}");
    let failure = dead.expect_err("a dead provider must fail the turn");
    let failure_text = failure.to_string();
    println!("FAULTLIVE leg A failure: {failure_text}");
    assert!(
        dead_elapsed < FAULT_WALL_LIMIT,
        "leg A must be bounded, took {dead_elapsed:.2?}"
    );

    // Leg B — the deadline requests cooperative cancellation. Same-child
    // join and original cleanup must settle before another Run is admitted.
    let stall = stalling_server().await;
    let stall_port = stall.port;
    let agent = Agent::from_config(fault_config(&format!("127.0.0.1:{stall_port}")))
        .await
        .expect("stall agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(fault_opts("fault-stalled-socket")),
        )
        .await
        .expect("stall session");
    let started = Instant::now();
    let stalled = session.send("reply with anything", None).await;
    let stall_elapsed = started.elapsed();
    println!("FAULTLIVE leg B (stalled socket) elapsed: {stall_elapsed:.2?}");
    let stall_failure = stalled.expect_err("a stalled provider must time out");
    let stall_text = stall_failure.to_string();
    println!("FAULTLIVE leg B failure: {stall_text}");
    assert!(
        stall_text.to_lowercase().contains("timed out")
            || stall_text.to_lowercase().contains("timeout"),
        "leg B must surface the timeout: {stall_text}"
    );
    assert!(
        stall_elapsed < FAULT_WALL_LIMIT,
        "leg B must be bounded, took {stall_elapsed:.2?}"
    );

    // Leg C — the SAME workspace on the REAL pinned model: the fault
    // legs poisoned nothing; the task continues.
    let mut real = load_pinned_layer_c_config();
    real.default_model = Some(support::layer_c_model::pinned_layer_c_model(&real));
    let agent = Agent::from_config(real).await.expect("real agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("fault-real-recovery")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_llm_api_timeout(180_000),
            ),
        )
        .await
        .expect("real recovery session");
    let started = Instant::now();
    let recovered = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "Reply with exactly the word RECOVERED and nothing else. Do not use any tools.",
            None,
        ),
    )
    .await
    .expect("real-model recovery turn timed out at the harness level");
    println!(
        "FAULTLIVE leg C (real model) elapsed: {:.2?}",
        started.elapsed()
    );
    let result = recovered.expect("the real model must recover after the fault legs");
    assert!(
        result.text.trim() == "RECOVERED",
        "the real recovery reply must follow the exact task"
    );
    println!("FAULTLIVE leg C reply: {}", result.text.trim());
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn consecutive_faults_keep_the_session_usable_and_the_real_model_recovers() {
    let workspace = tempfile::tempdir().expect("workspace");
    let stall = stalling_server().await;
    let stall_port = stall.port;
    let agent = Agent::from_config(fault_config(&format!("127.0.0.1:{stall_port}")))
        .await
        .expect("stall agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(fault_opts("fault-consecutive")),
        )
        .await
        .expect("stall session");

    // Leg A — the SAME session fails twice in a row, bounded both times:
    // one failure must not poison the session against the next attempt.
    for attempt in 1..=2 {
        let started = Instant::now();
        let failure = session
            .send(&format!("reply with anything (attempt {attempt})"), None)
            .await
            .expect_err("a stalled provider must fail the attempt");
        let elapsed = started.elapsed();
        println!(
            "FAULTLIVE2 consecutive attempt {attempt}: {:.2?} — {}",
            elapsed, failure
        );
        assert!(
            failure.to_string().to_lowercase().contains("timed out"),
            "attempt {attempt} must surface the timeout: {failure}"
        );
        assert!(
            elapsed < FAULT_WALL_LIMIT,
            "attempt {attempt} must be bounded, took {elapsed:.2?}"
        );
    }

    // Leg B — the real model on the SAME workspace recovers after the
    // consecutive failures: nothing the fault legs touched survives them.
    let mut real = load_pinned_layer_c_config();
    real.default_model = Some(support::layer_c_model::pinned_layer_c_model(&real));
    let agent = Agent::from_config(real).await.expect("real agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("fault2-real-recovery")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_llm_api_timeout(180_000),
            ),
        )
        .await
        .expect("real recovery session");
    let recovered = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "Reply with exactly the word RESUMED and nothing else. Do not use any tools.",
            None,
        ),
    )
    .await
    .expect("real-model recovery turn timed out at the harness level");
    let result = recovered.expect("the real model must recover after consecutive faults");
    assert_eq!(result.text.trim(), "RESUMED");
    println!("FAULTLIVE2 recovery reply: {}", result.text.trim());
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn concurrent_send_under_fault_hits_the_busy_contract_not_a_hang() {
    let workspace = tempfile::tempdir().expect("workspace");
    let stall = stalling_server().await;
    let stall_port = stall.port;
    let agent = Agent::from_config(fault_config(&format!("127.0.0.1:{stall_port}")))
        .await
        .expect("stall agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(fault_opts("fault-concurrent")),
        )
        .await
        .expect("stall session");

    // Observe actual transport entry before checking the busy contract.
    let (settled, (concurrent, concurrent_elapsed)) =
        tokio::join!(session.send("stalled turn", None), async {
            tokio::time::timeout(Duration::from_secs(5), stall.accepted.acquire())
                .await
                .expect("original provider connection")
                .unwrap()
                .forget();
            let started = Instant::now();
            let outcome = session.send("queued while stalled", None).await;
            (outcome, started.elapsed())
        },);
    println!(
        "FAULTLIVE3 concurrent send: {:.2?} — {:?}",
        concurrent_elapsed,
        concurrent.as_ref().err().map(|e| e.to_string())
    );
    assert!(
        matches!(concurrent, Err(CodeError::SessionBusy { .. })),
        "concurrent send must refuse the active original Run"
    );
    assert!(
        concurrent_elapsed < Duration::from_secs(1),
        "busy admission must be prompt, took {concurrent_elapsed:.2?}"
    );

    // The stalled turn itself settles bounded with the timeout error.
    let settled = settled.expect_err("stalled turn must fail");
    assert!(
        settled.to_string().to_lowercase().contains("timed out"),
        "stalled turn surfaces the timeout: {settled}"
    );
    let after = session
        .send("after the settle", None)
        .await
        .expect_err("still on the stalled endpoint — must fail");
    assert!(
        after.to_string().to_lowercase().contains("timed out"),
        "post-settle send stays bounded: {after}"
    );

    // Real-model recovery on the same workspace closes the leg.
    let mut real = load_pinned_layer_c_config();
    real.default_model = Some(support::layer_c_model::pinned_layer_c_model(&real));
    let agent = Agent::from_config(real).await.expect("real agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("fault3-real-recovery")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_llm_api_timeout(180_000),
            ),
        )
        .await
        .expect("real recovery session");
    let recovered = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "Reply with exactly the word DRAINED and nothing else. Do not use any tools.",
            None,
        ),
    )
    .await
    .expect("real-model recovery turn timed out at the harness level");
    let result = recovered.expect("the real model recovers after the concurrent-fault leg");
    assert_eq!(result.text.trim(), "DRAINED");
    println!("FAULTLIVE3 recovery reply: {}", result.text.trim());
}
