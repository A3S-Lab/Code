#![cfg(test)]

//! REL-TRAIN1: the release gate's real-model smoke. One ACP turn through
//! the actual binary against a real provider: `initialize`, `session/new`,
//! `session/prompt`, and the turn must end with `end_turn`.
//!
//! Ignored by default because it spends a real completion. Run it with
//! `just release-smoke` (or `cargo test -p a3s-code-acp --test
//! release_smoke -- --ignored`) and:
//!
//! - `A3S_RELEASE_SMOKE_CONFIG`: required ACL file with the provider;
//! - `A3S_RELEASE_SMOKE_MODEL`: optional `provider/model`;
//! - `A3S_RELEASE_SMOKE_BIN`: optional binary, for example a clean
//!   `cargo install` of the candidate. Defaults to the test build.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

const PROMPT: &str = "Reply with exactly the word READY and nothing else.";

async fn send(input: &mut ChildStdin, message: Value) {
    input
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();
    input.flush().await.unwrap();
}

/// Wait for the answer to `id`. Agent-to-client requests are refused, and
/// agent message chunks are collected into `reply`.
async fn answer(
    input: &mut ChildStdin,
    output: &mut Lines<BufReader<ChildStdout>>,
    id: u64,
    reply: &mut String,
) -> Value {
    loop {
        let line = output
            .next_line()
            .await
            .unwrap()
            .expect("ACP stdout ended before the answer");
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["method"] == "session/update" {
            let update = &message["params"]["update"];
            if update["sessionUpdate"] == "agent_message_chunk" {
                reply.push_str(update["content"]["text"].as_str().unwrap_or_default());
            }
        } else if message.get("method").is_some() && message.get("id").is_some() {
            send(
                input,
                json!({"jsonrpc":"2.0","id":message["id"],
                       "error":{"code":-32601,"message":"not supported by the release smoke"}}),
            )
            .await;
        } else if message["id"] == id {
            return message;
        }
    }
}

#[tokio::test]
#[ignore = "spends one real model completion; run through `just release-smoke`"]
async fn one_real_acp_turn_ends_with_end_turn() {
    let config = std::env::var_os("A3S_RELEASE_SMOKE_CONFIG")
        .expect("A3S_RELEASE_SMOKE_CONFIG must name the ACL file with a real provider");
    let bin = std::env::var_os("A3S_RELEASE_SMOKE_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_a3s-code-acp").into());
    let workspace = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(&bin);
    command
        .env_remove("A3S_CONFIG")
        .arg("--cwd")
        .arg(workspace.path())
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(model) = std::env::var_os("A3S_RELEASE_SMOKE_MODEL") {
        command.arg("--model").arg(model);
    }
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut reply = String::new();
    let started = Instant::now();

    let turn = tokio::time::timeout(Duration::from_secs(300), async {
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                   "params":{"protocolVersion":1,"clientCapabilities":{}}}),
        )
        .await;
        let initialized = answer(&mut input, &mut output, 1, &mut reply).await;
        assert!(initialized.get("result").is_some(), "{initialized}");

        let cwd = std::fs::canonicalize(workspace.path()).unwrap();
        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":2,"method":"session/new",
                   "params":{"cwd":cwd,"mcpServers":[]}}),
        )
        .await;
        let opened = answer(&mut input, &mut output, 2, &mut reply).await;
        let session = opened["result"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("session/new opened no session: {opened}"))
            .to_owned();
        let model = opened["result"]["models"]["currentModelId"].clone();

        send(
            &mut input,
            json!({"jsonrpc":"2.0","id":3,"method":"session/prompt",
                   "params":{"sessionId":session,
                             "prompt":[{"type":"text","text":PROMPT}]}}),
        )
        .await;
        let turn = answer(&mut input, &mut output, 3, &mut reply).await;
        (model, turn)
    })
    .await
    .expect("the real turn did not finish within 300 s");
    child.kill().await.unwrap();

    let (model, turn) = turn;
    eprintln!(
        "release smoke: model={model} stop={} reply={:?} seconds={:.1}",
        turn["result"]["stopReason"],
        reply.trim(),
        started.elapsed().as_secs_f64()
    );
    assert_eq!(turn["result"]["stopReason"], "end_turn", "{turn}");
    assert!(
        !reply.trim().is_empty(),
        "the turn streamed no reply: {turn}"
    );
}
