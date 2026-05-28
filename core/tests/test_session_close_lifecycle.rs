//! Cross-module integration tests for the session/agent close lifecycle.
//!
//! Unit tests in `core/src/agent_api/tests.rs` cover the isolated APIs.
//! This file exercises the *interaction* between session close, the
//! subagent task tracker, and the parent agent's session registry —
//! crossings that single-module unit tests cannot reach.
//!
//! Run with:
//!   cargo test --test test_session_close_lifecycle -- --nocapture

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_code_core::subagent_task_tracker::SubagentStatus;
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use tokio_util::sync::CancellationToken;

/// Minimal offline config — no real provider is contacted because every
/// test below avoids `send`/`stream`.
fn offline_test_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

/// IT-1: closing a session with a delegated subagent task in flight must
/// transition that task to Cancelled, fire its registered cancel token,
/// and — critically — a late `SubagentEnd` event from the cancelled child
/// loop must not regress the terminal status back to Completed.
///
/// This crosses the `session_close` → `subagent_task_tracker` →
/// `record_event` boundary that single-module unit tests cannot exercise.
#[tokio::test]
async fn close_with_subagent_in_flight_marks_task_cancelled_and_resists_regression() {
    let agent = Agent::from_config(offline_test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("it1-close-subagent");
    let session = agent
        .session("/tmp/it1-close-subagent-workspace", Some(opts))
        .expect("session");

    // Simulate the in-flight state that the built-in `task` tool produces:
    // a SubagentStart event, plus a registered cancellation token.
    let tracker = session.subagent_tracker();
    let task_id = "task-abc";
    let child_session_id = "child-xyz";
    let canceller = CancellationToken::new();

    tracker
        .record_event(&AgentEvent::SubagentStart {
            task_id: task_id.to_string(),
            session_id: child_session_id.to_string(),
            parent_session_id: session.id().to_string(),
            agent: "general".to_string(),
            description: "long-running synthetic task".to_string(),
        })
        .await;
    tracker.register_canceller(task_id, canceller.clone()).await;

    // Sanity: the task is visible as Running before close.
    let pending = session.pending_subagent_tasks().await;
    assert_eq!(pending.len(), 1, "pre-close pending list");
    assert_eq!(pending[0].task_id, task_id);
    assert_eq!(pending[0].status, SubagentStatus::Running);
    assert!(
        !canceller.is_cancelled(),
        "canceller must not be fired before close"
    );

    // Close the session — this is the cross-module action under test.
    session.close().await;
    assert!(session.is_closed(), "session must report closed");
    assert!(
        canceller.is_cancelled(),
        "subagent canceller must be fired by close()"
    );

    // The tracker view must show the task as Cancelled, and
    // pending_subagent_tasks() must drop it.
    let snapshot = session
        .subagent_task(task_id)
        .await
        .expect("snapshot still queryable after close");
    assert_eq!(snapshot.status, SubagentStatus::Cancelled);
    assert!(session.pending_subagent_tasks().await.is_empty());

    // Critical contract: a *late* SubagentEnd from the cancelled child loop
    // (success=true would be the worst case for status regression) must
    // NOT downgrade the terminal status back to Completed.
    tracker
        .record_event(&AgentEvent::SubagentEnd {
            task_id: task_id.to_string(),
            session_id: child_session_id.to_string(),
            agent: "general".to_string(),
            output: "would-have-succeeded".to_string(),
            success: true,
        })
        .await;
    let after_end = session
        .subagent_task(task_id)
        .await
        .expect("snapshot remains queryable");
    assert_eq!(
        after_end.status,
        SubagentStatus::Cancelled,
        "late SubagentEnd(success=true) must not regress Cancelled status"
    );
}

/// Minimal MCP server config — `enabled = false` so `connect_global_mcp`
/// does not actually spawn a subprocess. The presence of the entry still
/// causes `agent_bootstrap::connect_global_mcp` to construct a
/// `Some(McpManager)` (it only returns `None` when `mcp_servers` is
/// empty), which is what we need to exercise the MCP branch of
/// `Agent::close()`.
fn disabled_mcp_server(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: McpTransportConfig::Stdio {
            command: "/bin/true".to_string(),
            args: vec![],
        },
        enabled: false,
        env: std::collections::HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    }
}

/// IT-2: `Agent::close()` is idempotent and cleanly walks the
/// `global_mcp.list_connected()` branch even when there are no live
/// MCP connections — and is also safe when `global_mcp` is `None`.
///
/// We exercise both flavors (with and without `global_mcp`) so the
/// "if let Some(mcp)" arm in `agent_sessions::close_agent` is hit and
/// the no-`global_mcp` short-circuit is also covered.
#[tokio::test]
async fn agent_close_handles_global_mcp_branch_and_is_idempotent() {
    // Flavor A: no MCP at all — Agent::close() must short-circuit the
    // global_mcp branch.
    {
        let agent = Agent::from_config(offline_test_config()).await.unwrap();
        assert!(!agent.is_closed());
        agent.close().await;
        assert!(agent.is_closed());
        // Idempotent: second close is a no-op (no panic).
        agent.close().await;
        assert!(agent.is_closed());
    }

    // Flavor B: config carries a disabled MCP server entry. This makes
    // `agent_bootstrap::connect_global_mcp` return `Some(manager)` (the
    // manager is constructed because mcp_servers is non-empty) while
    // never opening a real connection. `list_connected()` is therefore
    // empty, and `Agent::close()` must traverse the branch cleanly.
    {
        let mut cfg = offline_test_config();
        cfg.mcp_servers = vec![disabled_mcp_server("offline-server")];
        let agent = Agent::from_config(cfg).await.unwrap();

        agent.close().await;
        assert!(agent.is_closed());

        // After close, the agent must reject new session creation —
        // proving close() ran the full close_agent path (not just the
        // MCP branch).
        let err = agent
            .session("/tmp/it2-post-close", None)
            .err()
            .expect("session() after close must error");
        let msg = err.to_string();
        assert!(
            msg.contains("closed") || msg.contains("Closed"),
            "post-close session() error must mention 'closed', got: {msg}"
        );
    }
}

/// IT-3: under concurrent creation + drop traffic, the agent session
/// registry must converge to *exactly* the IDs of sessions still held
/// by the caller. Single-threaded unit tests can't observe the
/// `std::sync::Mutex<HashMap<...>>` insert / drop / lazy-prune dance
/// under real parallelism.
///
/// Strategy:
/// 1. From N concurrent tasks on a multi-thread runtime, create one
///    session each.
/// 2. Drop half the sessions immediately; hold the other half.
/// 3. Wait for all tasks to settle.
/// 4. Assert `agent.list_sessions()` returns exactly the held IDs
///    (sorted, deduped).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_drop_prunes_registry_under_concurrency() {
    let agent = std::sync::Arc::new(Agent::from_config(offline_test_config()).await.unwrap());

    const N: usize = 32;

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let agent = std::sync::Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            let id = format!("it3-session-{i:02}");
            let opts = SessionOptions::new().with_session_id(&id);
            let session = agent
                .session(format!("/tmp/it3-ws-{i:02}"), Some(opts))
                .expect("session");

            // Drop the even-indexed sessions immediately so the registry
            // has to prune their Weak entries; hold the odd ones.
            if i % 2 == 0 {
                drop(session);
                None
            } else {
                Some((id, session))
            }
        }));
    }

    // Collect every held session so they outlive the assertion below.
    let mut held = Vec::new();
    for h in handles {
        if let Some(kept) = h.await.expect("task should not panic") {
            held.push(kept);
        }
    }

    let mut expected: Vec<String> = held.iter().map(|(id, _)| id.clone()).collect();
    expected.sort();

    let observed = agent.list_sessions().await;
    assert_eq!(
        observed, expected,
        "registry must contain exactly the IDs of still-held sessions"
    );

    // Now drop the held set and verify the registry collapses to empty
    // on the next access (lazy prune).
    drop(held);
    let after_drop = agent.list_sessions().await;
    assert!(
        after_drop.is_empty(),
        "after dropping all sessions the registry must prune to empty, got: {after_drop:?}"
    );
}
