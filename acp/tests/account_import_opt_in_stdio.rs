#![cfg(test)]
#![cfg(unix)]

//! SEC-ACCOUNT1: a default launch of the actual ACP binary reads no other
//! product's login. Cursor and Grok logins sit at their real discovery
//! paths under `HOME`; a sentinel `agent` program records whether the
//! Cursor importer ran. Only the host's opt-in (`import_accounts` in
//! `~/.a3s/config.acl` or `--import-accounts`) imports, and a repository
//! layer cannot opt in.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

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
    bin: PathBuf,
    token: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let state = root.path().join("state");
        let workspace = root.path().join("repo");
        let bin = root.path().join("bin");
        for dir in [&home.join(".a3s"), &state, &workspace, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join(".a3s/config.acl"), USER_ACL).unwrap();

        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(
            home.join(".cursor/cli-config.json"),
            r#"{"authInfo":{"email":"fixture@example.com","authId":"fixture-auth"}}"#,
        )
        .unwrap();
        let token = root.path().join("cursor-token");
        std::fs::write(&token, "cursor-fixture-bearer-5d1e").unwrap();
        let marker = root.path().join("cursor-agent-ran");
        let agent = bin.join("agent");
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\nprintf 'Available models\\n\\ncomposer-2.5 - Composer 2.5\\n'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();

        std::fs::create_dir_all(home.join(".grok")).unwrap();
        std::fs::write(
            home.join(".grok/auth.json"),
            r#"{"default":{"key":"grok-fixture-key","auth_mode":"api_key"}}"#,
        )
        .unwrap();

        Self {
            _root: root,
            home,
            state,
            workspace,
            bin,
            token,
            marker,
        }
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut path = vec![self.bin.clone()];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_a3s-code-acp"));
        command
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", std::env::join_paths(path).unwrap())
            .env("HOME", &self.home)
            .env("A3S_STATE_HOME", &self.state)
            // The keychain is never consulted in this test, opted in or not.
            .env("A3S_CURSOR_TOKEN_FILE", &self.token)
            .args(["--cwd", "."])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    fn cursor_agent_ran(&self) -> bool {
        self.marker.exists()
    }

    /// Model ids the real agent offers in `session/new`.
    async fn offered_models(&self, args: &[&str]) -> Vec<String> {
        let mut child = self.command(args).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        let workspace = std::fs::canonicalize(&self.workspace).unwrap();
        let initialized = rpc(
            &mut input,
            &mut output,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                   "params":{"protocolVersion":1,"clientCapabilities":{}}}),
        )
        .await;
        assert!(initialized.get("result").is_some(), "{initialized}");
        let opened = rpc(
            &mut input,
            &mut output,
            json!({"jsonrpc":"2.0","id":2,"method":"session/new",
                   "params":{"cwd":workspace,"mcpServers":[]}}),
        )
        .await;
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        opened["result"]["models"]["availableModels"]
            .as_array()
            .unwrap_or_else(|| panic!("session/new offers models: {opened}"))
            .iter()
            .filter_map(|model| model["modelId"].as_str().map(str::to_owned))
            .collect()
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

fn from_provider(models: &[String], provider: &str) -> bool {
    models
        .iter()
        .any(|id| id.starts_with(&format!("{provider}/")))
}

#[tokio::test]
async fn a_default_launch_reads_no_other_products_login() {
    let fixture = Fixture::new();
    let models = fixture.offered_models(&[]).await;
    assert!(from_provider(&models, "mock"), "{models:?}");
    assert!(
        !from_provider(&models, "cursor") && !from_provider(&models, "grok"),
        "a default launch imported a local account: {models:?}"
    );
    assert!(
        !fixture.cursor_agent_ran(),
        "a default launch ran the Cursor CLI to read the account catalog"
    );
}

#[tokio::test]
async fn a_repository_layer_cannot_opt_the_host_in() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.workspace.join(".a3s")).unwrap();
    std::fs::write(
        fixture.workspace.join(".a3s/config.acl"),
        "import_accounts = [\"cursor\", \"grok\"]\n",
    )
    .unwrap();
    let models = fixture.offered_models(&[]).await;
    assert!(
        !from_provider(&models, "cursor") && !from_provider(&models, "grok"),
        "a repository opted the host into an account import: {models:?}"
    );
    assert!(!fixture.cursor_agent_ran());
}

#[tokio::test]
async fn the_host_opt_in_imports_only_the_named_sources() {
    let fixture = Fixture::new();
    let models = fixture
        .offered_models(&["--import-accounts", "cursor"])
        .await;
    assert!(
        models.iter().any(|id| id == "cursor/composer-2.5"),
        "{models:?}"
    );
    assert!(!from_provider(&models, "grok"), "{models:?}");
    assert!(fixture.cursor_agent_ran());

    let fixture = Fixture::new();
    std::fs::write(
        fixture.home.join(".a3s/config.acl"),
        format!("{USER_ACL}import_accounts = [\"grok\"]\n"),
    )
    .unwrap();
    let models = fixture.offered_models(&[]).await;
    assert!(from_provider(&models, "grok"), "{models:?}");
    assert!(!from_provider(&models, "cursor"), "{models:?}");
    assert!(!fixture.cursor_agent_ran());
}
