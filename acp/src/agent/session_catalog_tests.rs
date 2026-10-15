#![cfg(test)]

use super::{test_launch, A3sCodeAgent};
use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{json, Value};

fn request(value: &Value) -> acp::ExtRequest {
    acp::ExtRequest::new(
        "x.ai/session/list",
        serde_json::value::to_raw_value(value)
            .expect("request")
            .into(),
    )
}

#[tokio::test]
async fn list_reads_workspace_facts_without_initializing_an_agent_or_store() {
    let root = tempfile::tempdir().expect("workspace");
    let workspace = root.path().canonicalize().expect("canonical workspace");
    let home = a3s_code_core::session_folder::prepare_session_home(&workspace, "catalog-session")
        .expect("session home")
        .expect("new folder");
    let fact = json!({
        "seq": 1, "type": "user.message", "key": "user:1", "cause": null,
        "payload": {"text": "修复列表中的会话\n完整原文"}
    });
    std::fs::write(home.join("catalog-session.jsonl"), format!("{fact}\n")).expect("fact log");
    let store = workspace.join("snapshot-store-must-not-be-created");
    let mut launch = test_launch(Some(store.clone()));
    launch.workspace = workspace.clone();
    let agent = A3sCodeAgent::new(launch);
    let response = agent
        .ext_method(request(
            &json!({"cwd": workspace, "limit": 30, "headless": "include"}),
        ))
        .await
        .expect("session list");
    let body: Value = serde_json::from_str(response.0.get()).expect("response JSON");
    assert_eq!(body["sessions"][0]["id"], "catalog-session");
    assert_eq!(
        body["sessions"][0]["firstPrompt"],
        "修复列表中的会话\n完整原文"
    );
    assert_eq!(body["sessions"][0]["numMessages"], 1);
    assert_eq!(body["_meta"]["x.ai/listScope"], "cwd");
    assert!(agent.core_agent.lock().await.is_none());
    assert!(agent.sessions.lock().await.is_empty());
    assert!(agent.config_mcp.lock().await.is_none());
    assert!(!store.exists());
}

fn agent_at(workspace: &std::path::Path) -> A3sCodeAgent {
    let mut launch = test_launch(Some(workspace.join("must-not-create-store")));
    launch.workspace = workspace.to_owned();
    A3sCodeAgent::new(launch)
}

fn legacy_fact(workspace: &std::path::Path, id: &str, text: &str) {
    let home = workspace.join(".a3s/effect-log");
    std::fs::create_dir_all(&home).expect("fixture directory");
    let fact = json!({"seq":1,"type":"user.message","key":"user:1","payload":{"text":text}});
    std::fs::write(home.join(format!("{id}.jsonl")), format!("{fact}\n")).expect("fixture fact");
}

async fn list(agent: &A3sCodeAgent, params: Value) -> Value {
    let response = agent
        .ext_method(request(&params))
        .await
        .expect("list response");
    serde_json::from_str(response.0.get()).expect("response JSON")
}

#[tokio::test]
async fn empty_workspace_does_not_create_state() {
    let root = tempfile::tempdir().expect("workspace");
    let agent = agent_at(root.path());
    let body = list(&agent, json!({})).await;
    assert_eq!(body["sessions"], json!([]));
    assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["incomplete"], false);
    assert!(!root.path().join(".a3s").exists());
    assert!(agent.core_agent.lock().await.is_none());
}

#[tokio::test]
async fn concurrent_inventory_is_refused_without_queuing_or_creating_state() {
    let root = tempfile::tempdir().expect("workspace");
    let agent = agent_at(root.path());
    let permit = agent
        .catalog_queries
        .clone()
        .try_acquire_owned()
        .expect("first scan admission");
    let error = agent
        .ext_method(request(&json!({})))
        .await
        .expect_err("busy inventory");
    assert_eq!(
        serde_json::to_value(error).expect("error"),
        serde_json::to_value(
            acp::Error::invalid_params().data("session catalog query already in progress")
        )
        .expect("expected error")
    );
    assert!(!root.path().join(".a3s").exists());
    assert!(agent.core_agent.lock().await.is_none());
    drop(permit);
    assert_eq!(list(&agent, json!({})).await["sessions"], json!([]));
}

#[test]
fn dropped_request_keeps_admission_until_its_blocking_worker_exits() {
    use std::future::Future;
    use std::task::{Context, Waker};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (started, ready) = tokio::sync::oneshot::channel();
    let root = tempfile::tempdir().expect("workspace");
    let agent = agent_at(root.path());
    runtime.block_on(async {
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            // Closing the sender also releases this test fixture on panic.
            let _ = released.recv();
        });
        ready.await.expect("blocking pool occupied");
        let mut pending = Box::pin(agent.ext_method(request(&json!({}))));
        let waker = Waker::noop();
        assert!(pending
            .as_mut()
            .poll(&mut Context::from_waker(waker))
            .is_pending());
        drop(pending);
        // The first filesystem worker is queued but its ACP waiter is gone.
        // A waiter-owned permit would admit this second request incorrectly.
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            agent.ext_method(request(&json!({}))),
        )
        .await
        .expect("busy refusal must not queue");
        assert!(second.is_err());
        release.send(()).expect("release blocking pool");
        blocker.await.expect("blocker exit");
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            agent.catalog_queries.clone().acquire_owned(),
        )
        .await
        .expect("worker releases admission")
        .expect("permit");
        drop(permit);
        assert_eq!(list(&agent, json!({})).await["sessions"], json!([]));
    });
    assert!(!root.path().join(".a3s").exists());
    assert!(agent.core_agent.try_lock().expect("agent lock").is_none());
}

#[tokio::test]
async fn foreign_and_missing_workspace_share_not_found_even_with_relaxation() {
    let root = tempfile::tempdir().expect("workspace");
    let foreign = tempfile::tempdir().expect("foreign workspace");
    legacy_fact(foreign.path(), "private", "must not be returned");
    let agent = agent_at(root.path());
    let mut errors = Vec::new();
    for cwd in [foreign.path().to_path_buf(), root.path().join("missing")] {
        let error = agent
            .ext_method(request(&json!({"cwd":cwd,"allowRelax":true})))
            .await
            .expect_err("not found");
        errors.push(serde_json::to_value(error).expect("error JSON"));
    }
    assert_eq!(errors[0], errors[1]);
    assert_eq!(
        errors[0],
        serde_json::to_value(acp::Error::resource_not_found(None)).expect("not found")
    );
    assert!(agent.core_agent.lock().await.is_none());
    assert!(!root.path().join(".a3s").exists());
}

#[tokio::test]
async fn invalid_fields_closed_sets_and_bounds_are_rejected() {
    let root = tempfile::tempdir().expect("workspace");
    let agent = agent_at(root.path());
    for params in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"limit":-1}),
        json!({"headless":"invented"}),
        json!({"allowRelax":"true"}),
        json!({"extra":true}),
        json!({"query":"x".repeat(4097)}),
        json!({"_meta":{"x.ai/facetFilters":{"kind":["invented"]}}}),
        json!({"_meta":{"unknown":true}}),
    ] {
        assert!(
            agent.ext_method(request(&params)).await.is_err(),
            "accepted {params}"
        );
    }
    let large = json!({"query":"x".repeat(1024*1024)});
    assert!(agent.ext_method(request(&large)).await.is_err());
    assert!(agent.core_agent.lock().await.is_none());
    assert!(!root.path().join(".a3s").exists());
}

#[tokio::test]
async fn query_limit_and_projection_use_legacy_facts() {
    let root = tempfile::tempdir().expect("workspace");
    legacy_fact(root.path(), "one", "different");
    legacy_fact(root.path(), "two", "精确原文查询");
    let agent = agent_at(root.path());
    let body = list(&agent, json!({"limit":1,"query":"精确原文"})).await;
    assert_eq!(body["sessions"].as_array().expect("rows").len(), 1);
    assert_eq!(body["sessions"][0]["id"], "two");
    assert_eq!(body["sessions"][0]["summary"], "精确原文查询");
    assert_eq!(body["sessions"][0]["source"], "local");
    assert_eq!(
        body["sessions"][0]["_meta"]["a3sCode/sessionCatalog"],
        "a3s.code.session-catalog.v1"
    );
    assert!(body["sessions"][0]["updatedAt"].as_str().is_some());
    assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["truncated"], false);
    let page = list(&agent, json!({"limit":1})).await;
    assert_eq!(page["_meta"]["a3sCode/sessionCatalog"]["truncated"], true);
    assert!(agent.core_agent.lock().await.is_none());
}

#[tokio::test]
async fn corrupt_logs_are_visible_as_partial_inventory() {
    let root = tempfile::tempdir().expect("workspace");
    legacy_fact(root.path(), "valid", "valid");
    std::fs::write(root.path().join(".a3s/effect-log/bad.jsonl"), b"corrupt").expect("bad log");
    let body = list(&agent_at(root.path()), json!({})).await;
    assert_eq!(body["sessions"].as_array().expect("rows").len(), 1);
    assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["incomplete"], true);
    assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["skippedEntries"], 1);
}

#[tokio::test]
async fn unknown_session_kind_is_never_invented_for_a_filtered_view() {
    let root = tempfile::tempdir().expect("workspace");
    legacy_fact(root.path(), "unknown-kind", "not a kind classifier");
    let agent = agent_at(root.path());
    for policy in ["exclude", "only"] {
        let body = list(&agent, json!({"headless":policy})).await;
        assert_eq!(body["sessions"], json!([]));
        assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["incomplete"], true);
        assert_eq!(
            body["_meta"]["a3sCode/sessionCatalog"]["unclassifiedPageEntries"],
            1
        );
    }
    let body = list(&agent, json!({"headless":"include"})).await;
    assert_eq!(body["sessions"].as_array().expect("rows").len(), 1);
}

#[tokio::test]
async fn chat_facet_does_not_read_or_mislabel_code_sessions() {
    let root = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(root.path().join(".a3s")).expect("state");
    std::fs::write(root.path().join(".a3s/sessions"), b"not a directory").expect("bad state");
    let body = list(
        &agent_at(root.path()),
        json!({"_meta":{"x.ai/facetFilters":{"kind":["chat"]}}}),
    )
    .await;
    assert_eq!(body["sessions"], json!([]));
    assert_eq!(body["_meta"]["a3sCode/sessionCatalog"]["incomplete"], false);
}

#[tokio::test]
async fn acp_wire_initialize_then_list_returns_persisted_facts() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let root = tempfile::tempdir().expect("workspace");
    legacy_fact(root.path(), "wire-session", "通过 ACP 查询事实");
    let agent = agent_at(root.path());
    tokio::task::LocalSet::new()
        .run_until(async {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let (read, write) = tokio::io::split(server);
            let (_connection, io) = acp::AgentSideConnection::new(
                agent,
                write.compat_write(),
                read.compat(),
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            let io_task = tokio::task::spawn_local(io);
            let (client_read, mut client_write) = tokio::io::split(client);
            let mut reader = BufReader::new(client_read);
            for (id, method, params) in [
                (
                    1,
                    "initialize",
                    json!({"protocolVersion":1,"clientCapabilities":{}}),
                ),
                (
                    2,
                    "_x.ai/session/list",
                    json!({"cwd":root.path(),"limit":30,"headless":"include"}),
                ),
            ] {
                let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
                client_write
                    .write_all(format!("{request}\n").as_bytes())
                    .await
                    .expect("wire request");
                let mut line = String::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    reader.read_line(&mut line),
                )
                .await
                .expect("bounded wire response")
                .expect("read response");
                let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");
                assert_eq!(response["id"], id);
                assert!(response.get("error").is_none(), "{response}");
                if id == 1 {
                    assert_eq!(response["result"]["_meta"]["a3sCode"], true);
                } else {
                    assert_eq!(response["result"]["sessions"][0]["id"], "wire-session");
                    assert_eq!(
                        response["result"]["sessions"][0]["firstPrompt"],
                        "通过 ACP 查询事实"
                    );
                    println!("session catalog wire response: {response}");
                }
            }
            io_task.abort();
            let _ = io_task.await;
        })
        .await;
    assert!(!root.path().join("must-not-create-store").exists());
    assert!(!root.path().join(".a3s/sessions").exists());
}
