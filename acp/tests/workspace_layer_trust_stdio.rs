#![cfg(test)]
#![cfg(unix)]

//! SEC-WSTRUST1: a cloned repository's `.a3s/config.acl` is untrusted
//! content. Opening the actual ACP binary inside it must not start a
//! process the repository declared until the user trusts that exact
//! declaration, and an edited declaration must lose that trust.

use std::path::{Path, PathBuf};
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

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    state: PathBuf,
    workspace: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let state = root.path().join("state");
        let workspace = root.path().join("cloned-repo");
        std::fs::create_dir_all(home.join(".a3s")).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(workspace.join(".a3s")).unwrap();
        std::fs::write(home.join(".a3s/config.acl"), USER_ACL).unwrap();
        let marker = root.path().join("repository-command-ran");
        let fixture = Self {
            _root: root,
            home,
            state,
            workspace,
            marker,
        };
        fixture.write_workspace_server("first");
        fixture
    }

    fn write_workspace_server(&self, revision: &str) {
        std::fs::write(
            self.workspace.join(".a3s/config.acl"),
            format!(
                r#"
mcp_servers "repo-tools" {{
  transport = "stdio"
  command = "/bin/sh"
  args = ["-c", "echo {revision} >> '{marker}'; sleep 30"]
}}
"#,
                marker = self.marker.display()
            ),
        )
        .unwrap();
    }

    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_a3s-code-acp"));
        command
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("A3S_STATE_HOME", &self.state)
            .env("A3S_DISABLE_CC_SWITCH", "1")
            .env("A3S_DISABLE_GROK_ACCOUNT", "1")
            .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    fn marker_lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.marker)
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    async fn trust_cli(&self, args: &[&str]) -> String {
        let output = self
            .command()
            .args(["--cwd", "."])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "trust command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

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
    tokio::time::timeout(Duration::from_secs(20), async {
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

/// Launch the agent in the workspace, open one session, and give any
/// configured MCP server time to start.
async fn open_session(fixture: &Fixture, extra: &[&std::ffi::OsStr]) -> (Child, Value) {
    let mut child = fixture
        .command()
        .args(["--cwd", "."])
        .args(extra)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let initialized = rpc(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize",
               "params":{"protocolVersion":1,"clientCapabilities":{}}}),
    )
    .await;
    assert!(initialized.get("result").is_some(), "{initialized}");
    let workspace = std::fs::canonicalize(&fixture.workspace).unwrap();
    let opened = rpc(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new",
               "params":{"cwd":workspace,"mcpServers":[]}}),
    )
    .await;
    assert!(
        opened["result"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{opened}"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Keep stdin open until the caller kills the child.
    std::mem::forget(input);
    (child, initialized)
}

fn pending_ids(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter(|line| line.contains("pending"))
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect()
}

async fn stop(mut child: Child) {
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

fn assert_no_workspace_trust_written(workspace: &Path) {
    assert!(
        !workspace.join(".a3s/workspace-mcp-trust.json").exists(),
        "trust must never be recorded inside the repository"
    );
}

#[tokio::test]
async fn untrusted_workspace_mcp_server_does_not_start_until_trusted() {
    let fixture = Fixture::new();

    let (child, initialized) = open_session(&fixture, &[]).await;
    stop(child).await;
    assert!(
        fixture.marker_lines().is_empty(),
        "opening an untrusted repository started its declared command"
    );
    let pending = &initialized["result"]["_meta"]["a3s"]["workspaceMcp"]["pending"];
    assert_eq!(pending.as_array().map(Vec::len), Some(1), "{initialized}");
    assert_eq!(pending[0]["name"], "repo-tools", "{initialized}");

    let listing = fixture.trust_cli(&["--list-workspace-mcp"]).await;
    let ids = pending_ids(&listing);
    assert_eq!(ids.len(), 1, "{listing}");
    assert!(listing.contains("repo-tools"), "{listing}");
    assert!(listing.contains("/bin/sh"), "{listing}");
    fixture
        .trust_cli(&["--trust-workspace-mcp", ids[0].as_str()])
        .await;
    assert_no_workspace_trust_written(&fixture.workspace);

    let (child, initialized) = open_session(&fixture, &[]).await;
    stop(child).await;
    // A command that never answers the MCP handshake is retried once by
    // the session sync, so count revisions rather than starts.
    let lines = fixture.marker_lines();
    assert!(
        !lines.is_empty() && lines.iter().all(|line| line == "first"),
        "the trusted declaration starts: {lines:?}"
    );
    let pending = &initialized["result"]["_meta"]["a3s"]["workspaceMcp"]["pending"];
    assert_eq!(pending.as_array().map(Vec::len), Some(0), "{initialized}");

    fixture.write_workspace_server("edited");
    let (child, _) = open_session(&fixture, &[]).await;
    stop(child).await;
    assert!(
        fixture.marker_lines().iter().all(|line| line == "first"),
        "an edited declaration must not inherit the earlier trust: {:?}",
        fixture.marker_lines()
    );
    assert_no_workspace_trust_written(&fixture.workspace);
}

#[tokio::test]
async fn explicit_config_mcp_servers_stay_host_trusted() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.workspace.join(".a3s/config.acl")).unwrap();
    let explicit = fixture.home.join("explicit.acl");
    std::fs::write(
        &explicit,
        format!(
            r#"{USER_ACL}
mcp_servers "host-tools" {{
  transport = "stdio"
  command = "/bin/sh"
  args = ["-c", "echo host >> '{marker}'; sleep 30"]
}}
"#,
            marker = fixture.marker.display()
        ),
    )
    .unwrap();
    let (child, _) = open_session(
        &fixture,
        &[std::ffi::OsStr::new("--config"), explicit.as_os_str()],
    )
    .await;
    stop(child).await;
    let lines = fixture.marker_lines();
    assert!(
        !lines.is_empty() && lines.iter().all(|line| line == "host"),
        "an explicit host config is consent: {lines:?}"
    );
}
