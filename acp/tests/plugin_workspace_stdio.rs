#![cfg(test)]
#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use a3s_use_reconcile::{
    files_sha256, PackageFile, PackageSpec, Payload, ReconcileStore, SurfaceKind, SurfaceSpec,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

async fn rpc(
    input: &mut ChildStdin,
    output: &mut Lines<BufReader<ChildStdout>>,
    request: Value,
) -> Value {
    input
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    input.flush().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = output.next_line().await.unwrap().expect("ACP stdout ended");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value.get("id") == request.get("id") {
                return value;
            }
        }
    })
    .await
    .expect("actual ACP process did not answer")
}

#[tokio::test]
async fn real_native_acp_binary_binds_relative_launch_cwd_and_refuses_foreign_protocol_roots() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let foreign = root.path().join("foreign");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&foreign).unwrap();
    let config = root.path().join("agent.acl");
    std::fs::write(
        &config,
        r#"
default_model = "mock/test"
providers "mock" {
  api_key = "not-a-model-credential"
  base_url = "http://127.0.0.1:9/v1"
  models "test" {}
}
"#,
    )
    .unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let files = vec![PackageFile {
        path: "echo".into(),
        bytes: b"#!/bin/sh\nprintf ready\n".to_vec(),
    }];
    let package = store
        .apply_sync(&PackageSpec {
            package_id: "acme/echo".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Tool,
                id: "echo".into(),
                sha256: files_sha256(&files).unwrap(),
                entry: Some("echo".into()),
                payload: Payload::Files(files),
            }],
        })
        .unwrap();
    let selected = store
        .publish_selection_sync("studio-installed-plugins", None, &[package.cursor()])
        .unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_a3s-code-acp"))
        .args(["--cwd", ".", "--config"])
        .arg(config)
        .arg("--plugin-store")
        .arg(root.path().join("generations"))
        .current_dir(&workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("A3S_DISABLE_CC_SWITCH", "1")
        .env("A3S_DISABLE_GROK_ACCOUNT", "1")
        .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let initialized = rpc(&mut input, &mut output, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}})).await;
    assert!(initialized.get("result").is_some(), "{initialized}");
    for (id, method) in [(2, "session/new"), (3, "session/load")] {
        let mut params =
            json!({"cwd":foreign,"mcpServers":[],"_meta":{"a3sPluginSelection":selected.cursor()}});
        if method == "session/load" {
            params["sessionId"] = "foreign-session".into();
        }
        let refused = rpc(
            &mut input,
            &mut output,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await;
        if method == "session/load" {
            assert_eq!(refused["error"]["code"], -32002, "{refused}");
            assert_eq!(
                refused["error"]["message"], "Resource not found",
                "{refused}"
            );
            assert!(refused["error"].get("data").is_none(), "{refused}");
        } else {
            assert_eq!(refused["error"]["code"], -32602, "{refused}");
            assert!(
                refused["error"]
                    .to_string()
                    .contains("native plugin workspace is not admitted"),
                "{refused}"
            );
        }
        assert!(!foreign.join(".a3s").exists());
    }
    let opened = rpc(&mut input, &mut output, json!({"jsonrpc":"2.0","id":4,"method":"session/new","params":{"cwd":std::fs::canonicalize(&workspace).unwrap(),"mcpServers":[],"_meta":{"a3sPluginSelection":selected.cursor()}}})).await;
    assert!(
        opened["result"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{opened}"
    );
    assert!(workspace.join(".a3s").is_dir());
    assert!(!foreign.join(".a3s").exists());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
