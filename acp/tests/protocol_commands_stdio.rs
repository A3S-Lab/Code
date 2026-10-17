#![cfg(test)]
#![cfg(unix)]

//! ACP-CMD1: the actual ACP binary announces its slash commands with a
//! standard `available_commands_update` after the session response, and an
//! extension method it does not implement is a JSON-RPC method-not-found
//! error, not a silent `null` success.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

const USER_ACL: &str = r#"
default_model = "mock/test"
providers "mock" {
  api_key = "not-a-model-credential"
  base_url = "http://127.0.0.1:9/v1"
  models "test" {}
}
"#;

struct Agent {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    /// Every line read so far, in arrival order.
    seen: Vec<Value>,
}

impl Agent {
    fn launch() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let state = root.path().join("state");
        let workspace = root.path().join("workspace");
        for dir in [home.join(".a3s"), state.clone(), workspace.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join(".a3s/config.acl"), USER_ACL).unwrap();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_a3s-code-acp"))
            .current_dir(&workspace)
            .args(["--cwd", "."])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("A3S_STATE_HOME", &state)
            .env("A3S_DISABLE_CC_SWITCH", "1")
            .env("A3S_DISABLE_GROK_ACCOUNT", "1")
            .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            workspace: std::fs::canonicalize(&workspace).unwrap(),
            _root: root,
            child,
            input,
            output,
            seen: Vec::new(),
        }
    }

    async fn send(&mut self, message: Value) {
        self.input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        self.input.flush().await.unwrap();
    }

    /// Read until a line satisfies `done`; return its index in `seen`.
    async fn read_until(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> usize {
        if let Some(index) = self.seen.iter().position(&done) {
            return index;
        }
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let line = self
                    .output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("ACP stdout ended");
                let value: Value = serde_json::from_str(&line).unwrap();
                let matched = done(&value);
                self.seen.push(value);
                if matched {
                    return self.seen.len() - 1;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {what} within 20 s; saw {:?}", self.seen))
    }

    async fn rpc(&mut self, request: Value) -> Value {
        let id = request["id"].clone();
        self.send(request).await;
        let index = self.read_until("response", |value| value["id"] == id).await;
        self.seen[index].clone()
    }

    async fn open_session(&mut self) -> (String, usize) {
        let initialized = self
            .rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                        "params":{"protocolVersion":1,"clientCapabilities":{}}}))
            .await;
        assert!(initialized.get("result").is_some(), "{initialized}");
        let workspace = self.workspace.clone();
        let opened = self
            .rpc(json!({"jsonrpc":"2.0","id":2,"method":"session/new",
                        "params":{"cwd":workspace,"mcpServers":[]}}))
            .await;
        let session_id = opened["result"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("{opened}"))
            .to_owned();
        let response_at = self
            .seen
            .iter()
            .rposition(|value| value["id"] == 2)
            .unwrap();
        (session_id, response_at)
    }

    async fn stop(mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}

fn is_commands_update(value: &Value, session_id: &str) -> bool {
    value["method"] == "session/update"
        && value["params"]["sessionId"] == session_id
        && value["params"]["update"]["sessionUpdate"] == "available_commands_update"
}

#[tokio::test]
async fn a_new_session_receives_the_available_commands_after_its_response() {
    let mut agent = Agent::launch();
    let (session_id, response_at) = agent.open_session().await;
    let update_at = agent
        .read_until("available_commands_update", |value| {
            is_commands_update(value, &session_id)
        })
        .await;
    assert!(
        update_at > response_at,
        "the update must follow the session/new response"
    );
    let commands = agent.seen[update_at]["params"]["update"]["availableCommands"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let goal = commands
        .iter()
        .find(|command| command["name"] == "goal")
        .unwrap_or_else(|| panic!("no goal command in {commands:?}"));
    assert!(goal["description"]
        .as_str()
        .is_some_and(|text| !text.is_empty()));
    assert_eq!(
        goal["input"]["hint"],
        "<objective> | status | pause | resume | clear"
    );
    agent.stop().await;
}

#[tokio::test]
async fn an_unknown_extension_method_is_method_not_found() {
    let mut agent = Agent::launch();
    let (session_id, _) = agent.open_session().await;
    let unknown = agent
        .rpc(
            json!({"jsonrpc":"2.0","id":3,"method":"_a3s/no-such-method",
                    "params":{"sessionId": session_id}}),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");
    assert!(unknown.get("result").is_none(), "{unknown}");

    let known = agent
        .rpc(json!({"jsonrpc":"2.0","id":4,"method":"_x.ai/session/info",
                    "params":{"sessionId": session_id}}))
        .await;
    assert!(known.get("error").is_none(), "{known}");
    agent.stop().await;
}
