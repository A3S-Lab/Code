use super::common::*;
use std::collections::BTreeMap;

const LOG_BYTES: &str = "ERROR LOG_FIXTURE\nERROR summary counts\nSummary complete\n";
const VICTIM_BYTES: &str = "preserve this fixture\n";
const SETTINGS: &str = r#"{"permissions":{
    "allow":["Bash","Read(*)","Search(*)"],
    "ask":["Bash(rm *)","Bash(*rm -rf*)"]
}}"#;
const CONFIG: &str = "[ui]\npermission_mode = \"ask\"\nremember_tool_approvals = true\n\n[marketplace]\ndefault_skills_installs_purged = true\n";
const REJECT_ROW: &str = "No, reject";

struct ToolTurn {
    expectation: AgentTurnExpectation,
    done: String,
}

struct FilenamePager {
    harness: PtyHarness,
    content: ContentController,
    log: PathBuf,
    victim: PathBuf,
    grants: BTreeMap<PathBuf, Vec<u8>>,
}

impl FilenamePager {
    async fn start() -> FilenamePager {
        assert!(
            std::env::var_os("PAGER_BINARY").is_some(),
            "supply PAGER_BINARY; implicit builds are forbidden"
        );
        let binary = pager_binary().expect("supplied pager binary");
        let rg = dunce::canonicalize(
            std::env::var_os("RG_BIN_PATH").expect("supply real rg via RG_BIN_PATH"),
        )
        .expect("real rg fixture exists");
        assert!(rg.is_file(), "RG_BIN_PATH must name a real executable file");
        let content = ContentController::start().await.expect("mock inference");
        let sandbox = content.sandbox();
        let cwd = sandbox.workspace();
        git2::Repository::init(cwd).expect("fixture repository");
        std::fs::create_dir_all(cwd.join(".claude")).expect("settings directory");
        std::fs::write(cwd.join(".claude/settings.json"), SETTINGS).expect("permission settings");
        std::fs::write(sandbox.grok_home().join("config.toml"), CONFIG).expect("Ask config");
        let log = cwd.join("sim.log");
        let victim = cwd.join("victim.txt");
        std::fs::write(&log, LOG_BYTES).expect("log fixture");
        std::fs::write(&victim, VICTIM_BYTES).expect("victim fixture");
        let baseline = sandbox
            .env()
            .into_iter()
            .find(|(key, _)| key == "PATH")
            .expect("sandbox PATH")
            .1;
        let path = std::env::join_paths(
            std::iter::once(rg.parent().expect("rg directory").to_path_buf())
                .chain(std::env::split_paths(&baseline)),
        )
        .expect("child PATH");
        let grants = grant_files(sandbox.grok_home());
        let harness = PtyHarness::spawn_with_content_env_ops_in_dir(
            &binary,
            DEFAULT_ROWS,
            DEFAULT_COLS,
            &content,
            &["--trust", "--no-leader", "--permission-mode", "default"],
            &[
                EnvOp::set_os(std::ffi::OsStr::new("PATH"), &path),
                EnvOp::set("SHELL", "/bin/bash"),
            ],
            Some(cwd),
        )
        .expect("Ask pager");
        let mut pager = FilenamePager {
            harness,
            content,
            log,
            victim,
            grants,
        };
        pager
            .harness
            .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
            .expect("welcome");
        pager
    }

    fn submit(&mut self, call_id: &str, name: &str, args: serde_json::Value) -> ToolTurn {
        let expectation = expect_tool_turn(&self.content, call_id, name, args.to_string());
        let done = format!("SETTLED_{call_id}");
        self.content.set_response(&done);
        self.harness
            .inject_keys(format!("run {call_id}\r").as_bytes())
            .expect("submit fixture turn");
        ToolTurn { expectation, done }
    }

    /// The tool runs without a permission prompt; returns its result.
    fn run(&mut self, call_id: &str, name: &str, args: serde_json::Value) -> String {
        let ToolTurn { expectation, done } = self.submit(call_id, name, args);
        self.harness
            .wait_until(
                "settled turn or permission prompt",
                Duration::from_secs(90),
                |h| h.contains_text(&done) || h.contains_text(REJECT_ROW),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "turn failed: {error}; screen:\n{}",
                    self.harness.screen_contents()
                )
            });
        assert!(
            !self.harness.contains_text(REJECT_ROW),
            "unexpected permission prompt for {call_id}; screen:\n{}",
            self.harness.screen_contents()
        );
        self.harness
            .wait_for_turn_idle(Duration::from_secs(30))
            .expect("turn finalized before the next fixture request");
        expectation.assert_satisfied();
        let output = self
            .content
            .requests()
            .iter()
            .find_map(|request| request.tool_results().remove(call_id));
        self.assert_unchanged();
        output.unwrap_or_else(|| panic!("no actual tool result for {call_id}"))
    }

    /// The permission prompt appears; rejecting it cancels the turn without running the tool.
    fn reject(&mut self, call_id: &str, name: &str, args: serde_json::Value) {
        let ToolTurn { expectation, .. } = self.submit(call_id, name, args);
        self.harness
            .wait_for_text(REJECT_ROW, Duration::from_secs(60))
            .expect("actual permission menu");
        let screen = self.harness.screen_contents();
        let row = screen
            .lines()
            .find(|line| line.contains(REJECT_ROW))
            .expect("reject row");
        let index = row
            .split_whitespace()
            .find_map(|word| word.parse::<usize>().ok())
            .expect("displayed option number");
        assert!((1..=9).contains(&index), "unexpected reject row: {row}");
        let keys = format!("{}{}", "\x1b[A".repeat(9), "\x1b[B".repeat(index - 1));
        self.harness
            .inject_keys(keys.as_bytes())
            .expect("navigate to displayed reject option");
        self.harness
            .wait_until("reject option selected", Duration::from_secs(10), |h| {
                h.screen_contents().lines().any(|line| {
                    line.contains(REJECT_ROW) && (line.contains("(●)") || line.contains("(•)"))
                })
            })
            .expect("reject row owns the selection before Enter");
        self.harness.inject_keys(b"\r").expect("reject once");
        self.harness
            .wait_for_text_absent(REJECT_ROW, Duration::from_secs(10))
            .expect("permission prompt closed");
        self.harness
            .wait_for_turn_idle(Duration::from_secs(30))
            .expect("rejected turn cancelled");
        assert!(
            self.harness.contains_text("permission was denied"),
            "missing cancellation banner for {call_id}; screen:\n{}",
            self.harness.screen_contents()
        );
        expectation.assert_satisfied();
        self.assert_unchanged();
    }

    fn assert_live_ask_mode(&mut self) {
        const F2: &[u8] = b"\x1bOQ";
        self.harness.inject_keys(F2).expect("open live settings");
        self.harness
            .wait_for_text("Settings", Duration::from_secs(10))
            .expect("settings modal");
        self.harness
            .inject_keys(b"/permission_mode")
            .expect("filter the live permission setting");
        self.harness
            .wait_until("live Ask mode", Duration::from_secs(10), |h| {
                h.screen_contents().lines().any(|line| {
                    line.split_once("Permission mode")
                        .is_some_and(|(_, value)| {
                            value.split_whitespace().any(|word| word == "Ask")
                        })
                })
            })
            .expect("live permission mode must be Ask, not Auto or always-approve");
        self.harness
            .inject_keys(F2)
            .expect("close without changing settings");
        self.harness
            .wait_for_text_absent("Permission mode", Duration::from_secs(10))
            .expect("settings dismissed");
        self.assert_unchanged();
    }

    fn assert_unchanged(&self) {
        assert_eq!(
            LOG_BYTES.as_bytes(),
            std::fs::read(&self.log).expect("log unchanged")
        );
        assert_eq!(
            VICTIM_BYTES.as_bytes(),
            std::fs::read(&self.victim).expect("victim unchanged")
        );
        assert_eq!(
            CONFIG,
            std::fs::read_to_string(self.content.sandbox().grok_home().join("config.toml"))
                .expect("mode config unchanged"),
        );
        assert_eq!(
            SETTINGS,
            std::fs::read_to_string(
                self.content
                    .sandbox()
                    .workspace()
                    .join(".claude/settings.json")
            )
            .expect("rules unchanged"),
        );
        assert_eq!(self.grants, grant_files(self.content.sandbox().grok_home()));
    }
}

fn grant_files(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let root = home.join("sessions");
    let mut files = BTreeMap::new();
    if !root.exists() {
        return files;
    }
    let mut pending = vec![(root, 0)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("session directory") {
            let entry = entry.expect("session entry");
            let kind = entry.file_type().expect("entry type");
            let path = entry.path();
            if kind.is_dir() && depth < 3 {
                pending.push((path, depth + 1));
            } else if kind.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("permission")
                && path.extension().is_some_and(|ext| ext == "toml")
            {
                files.insert(path.clone(), std::fs::read(path).expect("permission store"));
            }
        }
    }
    files
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires PAGER_BINARY and RG_BIN_PATH"]
async fn configured_bash_expansion_allows_read_and_rejects_rm() {
    let mut pager = FilenamePager::start().await;
    let log = pager.log.display().to_string();
    let literal = format!(
        r#"ls -lh "{log}"; rg -n "ERROR|FATAL" "{log}" | rg -v "summary|counts" | head -40; rg -n "Summary|ERROR :|FATAL :" "{log}" | tail -15"#
    );
    let output = pager.run(
        "literal_log",
        "run_terminal_command",
        json!({
            "command": literal,
            "description": "Read the fixture log through literal paths",
            "timeout": 10000,
        }),
    );
    for expected in ["exit: 0", "1:ERROR LOG_FIXTURE", "3:Summary complete"] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    pager.assert_live_ask_mode();
    let script = format!(
        r#"LOG="{log}"
echo "===== exists ====="
ls -lh "$LOG"
echo "===== errors ====="
rg -n "ERROR|FATAL" "$LOG" | rg -v "summary|counts" | head -40
echo "===== summary ====="
rg -n "Summary|ERROR :|FATAL :" "$LOG" | tail -15"#
    );
    let output = pager.run(
        "expanded_log",
        "run_terminal_command",
        json!({
            "command": script,
            "description": "Read the fixture log through a filename variable",
            "timeout": 10000,
        }),
    );
    for expected in ["exit: 0", "1:ERROR LOG_FIXTURE", "3:Summary complete"] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    for (id, name, args) in [
        (
            "remove_victim",
            "run_terminal_command",
            json!({
                "command": format!("rm \"{}\"", pager.victim.display()),
                "description": "Remove the sentinel fixture",
                "timeout": 10000,
            }),
        ),
        (
            "edit_victim",
            "search_replace",
            json!({"file_path": pager.victim, "old_string": VICTIM_BYTES, "new_string": "changed\n"}),
        ),
    ] {
        pager.reject(id, name, args);
    }
    pager.assert_live_ask_mode();
    quit_with_double_ctrl_c(&mut pager.harness);
    let exit = pager
        .harness
        .wait_for_exit_and_drain(Duration::from_secs(10), Duration::from_secs(2))
        .expect("graceful pager exit");
    assert_eq!(0, exit);
    pager.assert_unchanged();
}
