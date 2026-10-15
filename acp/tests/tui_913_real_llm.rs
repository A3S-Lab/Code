//! Real-LLM turns through the shipped 9.1.3 TUI.
//!
//! Runs when `A3S_CONFIG_FILE` points at the pinned layer-c ACL. The child
//! receives that path as `A3S_CONFIG` and does not rewrite the file.
//! `real_llm_tui_turn_shows_assistant_text` is one prose turn and is not
//! the 轮次 gate. Every live test refuses missing configuration; an unset
//! variable is never a passing live observation. The 轮次 and 长程任务 tests
//! use two independent defects.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test tui_913_real_llm -- --ignored --test-threads=1 --nocapture
//! ```

#![cfg(test)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

#[path = "tui_913_real_llm/fact_evidence.rs"]
mod fact_evidence;

#[path = "tui_913_real_llm/tui_recovery.rs"]
mod tui_recovery;

#[path = "tui_913_real_llm/tui_read.rs"]
mod tui_read;

#[path = "tui_913_real_llm/tui_coding.rs"]
mod tui_coding;

const PROMPT: &str = "Reply with one short sentence and do not call any tools.";
const PINNED_MODEL: &str = "boyue/deepseek-v4-flash";

fn tui_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("A3S_CODE_TUI_BIN") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/a3s-code-ui/target/debug/a3s-code-tui")
}

fn acp_binary() -> PathBuf {
    std::env::var_os("A3S_CODE_ACP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp")))
}

struct PtySession {
    parser: vt100::Parser,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    raw: Vec<u8>,
    cpr_replied_through: usize,
    secrets: Vec<String>,
}

fn open_pty(binary: &Path, cwd: &Path, env: &[(&str, &str)], secrets: Vec<String>) -> PtySession {
    let rows = 40;
    let cols = 120;
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");
    let mut command = CommandBuilder::new(binary);
    command.env_clear();
    // The ACL helper supplies credentials through these two environment
    // references. Preserve only that explicit handoff across the hermetic
    // pager environment; never print or copy the credentialed ACL.
    if std::env::var("A3S_CODE_ACL_ENV_PREPARED").as_deref() == Ok("1") {
        for key in ["A3S_OPENAI_API_KEY", "A3S_OPENAI_BASE_URL"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    command.arg("--cwd");
    command.arg(cwd);
    for (key, value) in env {
        command.env(key, value);
    }
    command.cwd(cwd);
    let child = pair.slave.spawn_command(command).expect("spawn tui");
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let writer = pair.master.take_writer().expect("writer");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    PtySession {
        parser: vt100::Parser::new(rows, cols, 1000),
        rx,
        writer,
        child,
        raw: Vec::new(),
        cpr_replied_through: 0,
        secrets,
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl PtySession {
    fn redact(&self, screen: &str) -> String {
        let mut out = screen.to_string();
        for secret in &self.secrets {
            if secret.len() >= 6 {
                out = out.replace(secret, "[redacted]");
            }
        }
        out
    }

    fn drain(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            self.raw.extend_from_slice(&bytes);
            self.parser.process(&bytes);
        }
        let query = b"\x1b[6n";
        if let Some(at) = self
            .raw
            .windows(query.len())
            .rposition(|window| window == query)
        {
            if at >= self.cpr_replied_through {
                self.cpr_replied_through = self.raw.len();
                let _ = self.writer.write_all(b"\x1b[12;1R");
                let _ = self.writer.flush();
            }
        }
    }

    fn screen(&mut self) -> String {
        self.drain();
        self.parser.screen().contents()
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if let Err(error) = self.writer.write_all(bytes) {
            let screen = self.screen();
            panic!("pty write failed: {error}\n{}", self.redact(&screen));
        }
        self.writer.flush().ok();
    }

    fn type_text(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    fn enter(&mut self) {
        self.write_bytes(b"\r");
    }

    fn clear_line(&mut self) {
        self.write_bytes(b"\x15");
    }

    fn wait_for(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.screen();
            if last.contains("Do you trust the contents of this directory") {
                self.type_text("y");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if screen_has(&last, needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let redacted = self.redact(&last);
        panic!("timed out waiting for {needle:?}\n{redacted}");
    }
}

/// The composer wraps a long prompt across rows and draws a box between them.
/// A match ignores that chrome so the typed text still counts as present.
fn screen_has(screen: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    if screen.contains(needle) {
        return true;
    }
    let flat: String = screen
        .chars()
        .map(|ch| match ch {
            '│' | '╭' | '╮' | '╰' | '╯' | '─' | '┄' | '┆' => ' ',
            _ => ch,
        })
        .collect();
    let screen_words = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let needle_words = needle.split_whitespace().collect::<Vec<_>>().join(" ");
    screen_words.contains(&needle_words)
}

const ERROR_BANNERS: &[&str] = &[
    "Turn failed",
    "Connection failed",
    "Request failed",
    "Server error",
    "Rate limited",
    "No response from the model",
    "Empty response",
    "Authentication temporarily unavailable",
    "Couldn't read the response",
    "Couldn't switch model",
    "Service Unavailable",
];

fn banner(screen: &str) -> Option<&str> {
    ERROR_BANNERS
        .iter()
        .copied()
        .find(|needle| screen.contains(needle))
}

fn letter_count(line: &str) -> usize {
    line.chars().filter(|ch| ch.is_alphabetic()).count()
}

/// A reply is prose: several words, and not a path, context meter, or clock.
fn is_prose(line: &str) -> bool {
    if line.contains('/') || line.contains('\\') {
        return false;
    }
    let words = line
        .split_whitespace()
        .filter(|word| letter_count(word) >= 3)
        .count();
    words >= 3
}

fn is_clock_token(token: &str) -> bool {
    let mut parts = token.split(':');
    let hour = parts.next();
    let minute = parts.next();
    match (hour, minute, parts.next()) {
        (Some(hour), Some(minute), None) => {
            !hour.is_empty()
                && hour.len() <= 2
                && hour.chars().all(|ch| ch.is_ascii_digit())
                && minute.len() == 2
                && minute.chars().all(|ch| ch.is_ascii_digit())
        }
        _ => false,
    }
}

/// Drop a trailing pager clock so `TOKEN  10:01 PM` is the same line a minute later.
fn without_clock(line: &str) -> String {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if words
        .last()
        .is_some_and(|word| word.eq_ignore_ascii_case("am") || word.eq_ignore_ascii_case("pm"))
    {
        words.pop();
    }
    if words.last().is_some_and(|word| is_clock_token(word)) {
        words.pop();
    }
    words.join(" ")
}

/// A one-token assistant line still counts when it is the planted marker.
/// Paths and the user prompt row (which carries `❯`) do not.
fn is_marker_reply(line: &str, marker: &str) -> bool {
    !marker.is_empty()
        && !line.contains('/')
        && !line.contains('\\')
        && !line.contains('❯')
        && line.split_whitespace().any(|word| word.contains(marker))
}

fn is_chrome(line: &str) -> bool {
    const CHROME: &[&str] = &[
        "Waiting",
        "[stop]",
        "send a message to interrupt",
        "Thinking",
        "Shift+Tab",
        "Ctrl+",
        "Do you trust",
        "shortcuts",
        "Connecting",
        "Esc to",
        "A3S Code",
        "Thanks for trying",
        "New worktree",
        "Resume session",
        "Thought for",
        "Worked for",
        "Changelog",
        "clipboard",
        "ctrl+",
        "to paste",
    ];
    CHROME.iter().any(|needle| line.contains(needle))
}

fn fresh_assistant_lines(before: &str, after: &str, prompt: &str) -> Vec<String> {
    collect_fresh_lines(before, after, prompt, None)
}

fn collect_fresh_lines(
    before: &str,
    after: &str,
    prompt: &str,
    marker: Option<&str>,
) -> Vec<String> {
    let prior: std::collections::HashSet<String> = before
        .lines()
        .map(without_clock)
        .filter(|line| !line.is_empty())
        .collect();
    after
        .lines()
        .filter_map(|raw| {
            let body = without_clock(raw);
            if body.is_empty()
                || prior.contains(&body)
                || (!prompt.is_empty() && body.contains(prompt))
                || body.contains(PINNED_MODEL)
                || body.contains('❯')
                || is_chrome(&body)
                || banner(&body).is_some()
            {
                return None;
            }
            let marked = marker.is_some_and(|token| is_marker_reply(&body, token));
            if is_prose(&body) || marked {
                Some(body)
            } else {
                None
            }
        })
        .collect()
}

fn fresh_matching(
    before: &str,
    after: &str,
    prompt: &str,
    pred: impl Fn(&str) -> bool,
) -> Vec<String> {
    let prior: std::collections::HashSet<String> = before
        .lines()
        .map(without_clock)
        .filter(|line| !line.is_empty())
        .collect();
    after
        .lines()
        .filter_map(|raw| {
            let body = without_clock(raw);
            if body.is_empty()
                || prior.contains(&body)
                || (!prompt.is_empty() && body.contains(prompt))
                || body.contains(PINNED_MODEL)
                || body.contains('❯')
                || body.contains('/')
                || body.contains('\\')
                || is_chrome(&body)
                || banner(&body).is_some()
                || !pred(&body)
            {
                return None;
            }
            Some(body)
        })
        .collect()
}

fn assert_screen_clean(pty: &PtySession, screen: &str) {
    let redacted = pty.redact(screen);
    assert!(
        banner(screen).is_none(),
        "provider error banner\n{redacted}"
    );
    let lower = screen.to_ascii_lowercase();
    assert!(
        !lower.contains("api_key") && !screen.contains("sk-") && !screen.contains("Bearer "),
        "screen showed a credential\n{redacted}"
    );
    for secret in &pty.secrets {
        if secret.len() >= 6 {
            assert!(
                !screen.contains(secret.as_str()),
                "screen showed a credential\n{redacted}"
            );
        }
    }
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_turn_shows_assistant_text() {
    let config_value = std::env::var_os("A3S_CONFIG_FILE")
        .filter(|value| !value.is_empty())
        .expect("A3S_CONFIG_FILE is unset; refusing to pass");
    let config = PathBuf::from(config_value);
    assert!(config.is_file(), "A3S_CONFIG_FILE is not a file");
    let parsed = a3s_code_core::CodeConfig::from_file(&config)
        .unwrap_or_else(|_| panic!("credentialed ACL could not be loaded"));
    assert_eq!(
        parsed.default_model.as_deref(),
        Some(PINNED_MODEL),
        "pinned ACL default_model is not {PINNED_MODEL}"
    );
    let before_acl = std::fs::read(&config).expect("acl bytes");
    let mut secrets = Vec::new();
    for provider in &parsed.providers {
        secrets.extend(provider.api_key.iter().cloned());
        secrets.extend(provider.base_url.iter().cloned());
        secrets.extend(provider.headers.values().cloned());
        for model in &provider.models {
            secrets.extend(model.api_key.iter().cloned());
            secrets.extend(model.base_url.iter().cloned());
            secrets.extend(model.headers.values().cloned());
        }
    }

    let root = tempfile::tempdir().expect("temp");
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(home.join("grok")).expect("home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("sentinel.txt"), b"unchanged").expect("sentinel");

    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "build a3s-code-tui before this test: {}",
        binary.display()
    );
    let acp = acp_binary();
    let env = vec![
        ("A3S_ACP_AGENT_BIN".to_string(), acp.display().to_string()),
        ("A3S_CONFIG".to_string(), config.display().to_string()),
        ("A3S_DISABLE_CC_SWITCH".to_string(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".to_string(), "1".into()),
        ("A3S_DISABLE_CURSOR_ACCOUNT".to_string(), "1".into()),
        ("HOME".to_string(), home.display().to_string()),
        (
            "GROK_HOME".to_string(),
            home.join("grok").display().to_string(),
        ),
        (
            "TMPDIR".to_string(),
            std::env::temp_dir().display().to_string(),
        ),
        ("TERM".to_string(), "xterm-256color".into()),
        (
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/opt/homebrew/bin".into(),
        ),
    ];
    let refs: Vec<(&str, &str)> = env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut pty = open_pty(&binary, &workspace, &refs, secrets);
    let ready = pty.wait_for("DeepSeek V4 Flash", Duration::from_secs(45));
    assert!(
        ready.contains("boyue/"),
        "composer missed the pinned provider\n{}",
        pty.redact(&ready)
    );
    assert_screen_clean(&pty, &ready);
    let before = pty.screen();
    pty.clear_line();
    pty.type_text(PROMPT);
    let typed = pty.wait_for(PROMPT, Duration::from_secs(3));
    assert_screen_clean(&pty, &typed);
    pty.enter();

    let deadline = Instant::now() + Duration::from_secs(420);
    let mut last = String::new();
    let mut assistant = Vec::new();
    while Instant::now() < deadline {
        last = pty.screen();
        if last.contains("Do you trust the contents of this directory") {
            pty.type_text("y");
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        if let Some(hit) = banner(&last) {
            panic!("turn/provider error banner {hit}\n{}", pty.redact(&last));
        }
        assistant = fresh_assistant_lines(&before, &last, PROMPT);
        if !assistant.is_empty() {
            std::thread::sleep(Duration::from_millis(800));
            last = pty.screen();
            if let Some(hit) = banner(&last) {
                panic!("turn/provider error banner {hit}\n{}", pty.redact(&last));
            }
            assistant = fresh_assistant_lines(&before, &last, PROMPT);
            if !assistant.is_empty() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    assert_screen_clean(&pty, &last);
    assert!(
        !assistant.is_empty(),
        "screen showed no assistant text\n{}",
        pty.redact(&last)
    );
    let shown = assistant.join(" ");
    assert!(letter_count(&shown) >= 8, "assistant text was empty");
    let sentinel = std::fs::read(workspace.join("sentinel.txt")).expect("sentinel");
    assert_eq!(sentinel, b"unchanged");
    let after_acl = std::fs::read(&config).expect("acl bytes");
    assert_eq!(before_acl, after_acl, "the pinned ACL was rewritten");
    eprintln!("ORACLE case=real-llm assistant={}", pty.redact(&shown));
}

struct LiveSession {
    _root: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
    before_acl: Vec<u8>,
    launch_acl: Option<(PathBuf, Vec<u8>)>,
    prompt_trace: Option<PathBuf>,
    pty: PtySession,
}

fn open_live() -> Option<LiveSession> {
    open_live_with_verification(false)
}

fn open_live_with_verification(coding: bool) -> Option<LiveSession> {
    let config_value = std::env::var_os("A3S_CONFIG_FILE").filter(|value| !value.is_empty())?;
    let config = PathBuf::from(config_value);
    assert!(config.is_file(), "A3S_CONFIG_FILE is not a file");
    let parsed = a3s_code_core::CodeConfig::from_file(&config)
        .unwrap_or_else(|_| panic!("credentialed ACL could not be loaded"));
    assert_eq!(
        parsed.default_model.as_deref(),
        Some(PINNED_MODEL),
        "pinned ACL default_model is not {PINNED_MODEL}"
    );
    let before_acl = std::fs::read(&config).expect("acl bytes");
    let mut secrets = Vec::new();
    for provider in &parsed.providers {
        secrets.extend(provider.api_key.iter().cloned());
        secrets.extend(provider.base_url.iter().cloned());
        secrets.extend(provider.headers.values().cloned());
        for model in &provider.models {
            secrets.extend(model.api_key.iter().cloned());
            secrets.extend(model.base_url.iter().cloned());
            secrets.extend(model.headers.values().cloned());
        }
    }

    let root = tempfile::tempdir().expect("temp");
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(home.join("grok")).expect("home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("sentinel.txt"), b"unchanged").expect("sentinel");

    // Exercise the public host ACL entry through the actual ACP process.
    // This is not SessionOptions injection or a workspace-owned declaration.
    let launch_acl = coding.then(|| {
        use a3s_acl::{Block, Value};
        let source = std::str::from_utf8(&before_acl).expect("ACL UTF-8");
        let mut document = a3s_acl::parse_acl(source).expect("host ACL document");
        document.blocks.push(Block {
            name: "verification_contract".into(),
            labels: vec!["dual-defect".into()],
            blocks: Vec::new(),
            attributes: std::collections::HashMap::from([
                (
                    "workspace".into(),
                    Value::String(workspace.to_string_lossy().into_owned()),
                ),
                ("command".into(), Value::String("python3 gate.py".into())),
                (
                    "covers".into(),
                    Value::List(
                        ["rate.py", "fee.py", "runs.txt"]
                            .into_iter()
                            .map(|path| Value::String(path.into()))
                            .collect(),
                    ),
                ),
            ]),
        });
        let text = a3s_acl::generate_acl(&document);
        let path = root.path().join("host-config.acl");
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&path)
            .expect("private host ACL")
            .write_all(text.as_bytes())
            .expect("host ACL bytes");
        (path, text.into_bytes())
    });
    let launch_config = launch_acl.as_ref().map(|(path, _)| path).unwrap_or(&config);

    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "build a3s-code-tui before this test: {}",
        binary.display()
    );
    let acp = acp_binary();
    let prompt_trace = coding.then(|| root.path().join("prompt-trace.jsonl"));
    let proxy = coding.then(|| {
        let path = root.path().join("acp-trace-proxy");
        std::fs::write(&path, include_str!("tui_913_real_llm/acp_trace_proxy.py"))
            .expect("test ACP relay");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("private executable relay");
        }
        path
    });
    let mut env = vec![
        (
            "A3S_ACP_AGENT_BIN".to_string(),
            proxy.as_ref().unwrap_or(&acp).display().to_string(),
        ),
        (
            "A3S_CONFIG".to_string(),
            launch_config.display().to_string(),
        ),
        ("A3S_DISABLE_CC_SWITCH".to_string(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".to_string(), "1".into()),
        ("A3S_DISABLE_CURSOR_ACCOUNT".to_string(), "1".into()),
        ("HOME".to_string(), home.display().to_string()),
        (
            "GROK_HOME".to_string(),
            home.join("grok").display().to_string(),
        ),
        (
            "TMPDIR".to_string(),
            std::env::temp_dir().display().to_string(),
        ),
        ("TERM".to_string(), "xterm-256color".into()),
        (
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/opt/homebrew/bin".into(),
        ),
    ];
    if let Some(trace) = &prompt_trace {
        env.push((
            "A3S_CODE_TUI_TEST_ACP_REAL".into(),
            acp.display().to_string(),
        ));
        env.push((
            "A3S_CODE_TUI_TEST_ACP_TRACE".into(),
            trace.display().to_string(),
        ));
    }
    let refs: Vec<(&str, &str)> = env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut pty = open_pty(&binary, &workspace, &refs, secrets);
    let ready = pty.wait_for("DeepSeek V4 Flash", Duration::from_secs(45));
    assert!(
        ready.contains("boyue/"),
        "composer missed the pinned provider\n{}",
        pty.redact(&ready)
    );
    assert_screen_clean(&pty, &ready);
    Some(LiveSession {
        _root: root,
        home,
        workspace,
        config,
        before_acl,
        launch_acl,
        prompt_trace,
        pty,
    })
}

fn answer_dialog(pty: &mut PtySession, screen: &str) -> bool {
    if screen.contains("Do you trust the contents of this directory") {
        pty.type_text("y");
        std::thread::sleep(Duration::from_millis(200));
        return true;
    }
    if screen.contains("Yes, proceed") {
        pty.type_text("1");
        std::thread::sleep(Duration::from_millis(400));
        return true;
    }
    false
}

fn submit_prompt(pty: &mut PtySession, prompt: &str) {
    pty.clear_line();
    pty.type_text(prompt);
    let typed = pty.wait_for(prompt, Duration::from_secs(5));
    assert_screen_clean(pty, &typed);
    pty.enter();
}

fn assert_acl_unchanged(session: &LiveSession) {
    let after_acl = std::fs::read(&session.config).expect("acl bytes");
    assert_eq!(
        session.before_acl, after_acl,
        "the pinned ACL was rewritten"
    );
    if let Some((path, before)) = &session.launch_acl {
        assert_eq!(&std::fs::read(path).expect("host contract ACL"), before);
    }
}

const RATE_BUG: &str = "return count * 3";
const FEE_BUG: &str = "return count + 11";
const PASS_LINE: &str = "DUAL-GATE-7K";
const FAIL_LINE: &str = "DUAL-HOLD";
const DIAG_PROMPT: &str =
    "Read rate.py, fee.py, and gate.py. Name both wrong returns. Do not edit.";
const PARTIAL_PROMPT: &str =
    "Fix only rate.py. Run exactly `python3 gate.py`. If denied, request escalation. Quote output.";
const SECOND_PROMPT: &str =
    "Fix fee.py. Run exactly `python3 gate.py`. If denied, request escalation. Quote output.";
const RUN_PROMPT: &str =
    "Run exactly `python3 gate.py`. If denied, request escalation. Quote its output.";
const LONG_PROMPT: &str = "Run exactly `python3 gate.py` first and after each fix. Fix rate.py then fee.py. Quote output.";
const RATE_PY: &str = "def sticker_price(count):\n    return count * 3\n";
const FEE_PY: &str = "def parcel_fee(count):\n    return count + 11\n";
const RATE_FIXED: &str = "def sticker_price(count):\n    return count * 8\n";
const FEE_FIXED: &str = "def parcel_fee(count):\n    return count * 5\n";
const GATE_PY: &str = "\
import sys\n\
sys.dont_write_bytecode = True\n\
import importlib\n\
import fee\n\
import rate\n\
\n\
importlib.reload(rate)\n\
importlib.reload(fee)\n\
sticker_price = rate.sticker_price\n\
parcel_fee = fee.parcel_fee\n\
price_ok = sticker_price(4) == 32 and sticker_price(5) == 40\n\
fee_ok = parcel_fee(4) == 20 and parcel_fee(3) == 15\n\
status = \"HOLD\" if not (price_ok and fee_ok) else \"GATE-7K\"\n\
line = \"DUAL-\" + status\n\
print(line, flush=True)\n\
open(\"runs.txt\", \"a\", encoding=\"utf-8\").write(line + \"\\n\")\n\
raise SystemExit(0 if price_ok and fee_ok else 1)\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefectState {
    Both,
    RateOnly,
    FeeOnly,
    Clear,
}

fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let marker = format!("def {name}");
    let Some(start) = src.find(&marker) else {
        return src;
    };
    let rest = &src[start + marker.len()..];
    let body = rest.find('\n').map(|at| &rest[at + 1..]).unwrap_or(rest);
    match body.find("\ndef ") {
        Some(next) => &body[..next],
        None => body,
    }
}

fn has_factor(body: &str, factor: u32) -> bool {
    let compact: String = body.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = compact.as_bytes();
    let digits = factor.to_string();
    let digits = digits.as_bytes();
    star_then_number(bytes, digits) || number_then_star(bytes, digits)
}

fn star_then_number(bytes: &[u8], digits: &[u8]) -> bool {
    let needle_len = 1 + digits.len();
    if bytes.len() < needle_len {
        return false;
    }
    for start in 0..=bytes.len() - needle_len {
        if bytes[start] == b'*' && &bytes[start + 1..start + needle_len] == digits {
            let after = start + needle_len;
            if after == bytes.len() || !bytes[after].is_ascii_digit() {
                return true;
            }
        }
    }
    false
}

fn number_then_star(bytes: &[u8], digits: &[u8]) -> bool {
    let needle_len = digits.len() + 1;
    if bytes.len() < needle_len {
        return false;
    }
    for start in 0..=bytes.len() - needle_len {
        if &bytes[start..start + digits.len()] == digits && bytes[start + digits.len()] == b'*' {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
            if before_ok {
                return true;
            }
        }
    }
    false
}

fn price_fixed(src: &str) -> bool {
    let body = fn_body(src, "sticker_price");
    !body.contains(RATE_BUG) && has_factor(body, 8)
}

fn fee_fixed(src: &str) -> bool {
    let body = fn_body(src, "parcel_fee");
    !body.contains(FEE_BUG) && has_factor(body, 5)
}

fn classify(rate: &str, fee: &str) -> DefectState {
    match (price_fixed(rate), fee_fixed(fee)) {
        (false, false) => DefectState::Both,
        (false, true) => DefectState::RateOnly,
        (true, false) => DefectState::FeeOnly,
        (true, true) => DefectState::Clear,
    }
}

fn predicted_line(state: DefectState) -> &'static str {
    match state {
        DefectState::Clear => PASS_LINE,
        DefectState::Both | DefectState::RateOnly | DefectState::FeeOnly => FAIL_LINE,
    }
}

fn read_workspace_file(workspace: &Path, name: &str) -> String {
    std::fs::read_to_string(workspace.join(name)).unwrap_or_default()
}

fn read_defect_state(workspace: &Path) -> DefectState {
    classify(
        &read_workspace_file(workspace, "rate.py"),
        &read_workspace_file(workspace, "fee.py"),
    )
}

fn write_shop_fixture(workspace: &Path) {
    std::fs::write(workspace.join("rate.py"), RATE_PY).expect("rate.py");
    std::fs::write(workspace.join("fee.py"), FEE_PY).expect("fee.py");
    std::fs::write(workspace.join("gate.py"), GATE_PY).expect("gate.py");
}

fn run_gate(workspace: &Path) -> String {
    // A private directory so this call does not append the session `runs.txt`.
    // The checker source has no skip switch the model can copy.
    let probe = tempfile::tempdir().expect("probe");
    for name in ["rate.py", "fee.py", "gate.py"] {
        std::fs::copy(workspace.join(name), probe.path().join(name)).expect(name);
    }
    let output = std::process::Command::new("/usr/bin/python3")
        .env_clear()
        .arg("gate.py")
        .current_dir(probe.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 gate.py");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        (stdout == PASS_LINE && output.status.code() == Some(0))
            || (stdout == FAIL_LINE && output.status.code() == Some(1)),
        "python3 gate.py status={} stdout={stdout:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn prompts_omit_secrets_and_defects() {
    for prompt in [
        DIAG_PROMPT,
        PARTIAL_PROMPT,
        SECOND_PROMPT,
        RUN_PROMPT,
        LONG_PROMPT,
    ] {
        assert!(prompt.len() < 100, "prompt wraps the composer: {prompt}");
        assert!(!prompt.contains(PASS_LINE), "{prompt}");
        assert!(!prompt.contains(FAIL_LINE), "{prompt}");
        assert!(!prompt.contains(RATE_BUG), "{prompt}");
        assert!(!prompt.contains(FEE_BUG), "{prompt}");
        assert!(!prompt.contains("GATE-7K"), "{prompt}");
    }
    assert!(!GATE_PY.contains(PASS_LINE));
    assert!(!GATE_PY.contains("GATE_SKIP_LOG"));
    assert!(!GATE_PY.contains(FAIL_LINE));
}

fn exactly_one(state: DefectState) -> bool {
    matches!(state, DefectState::RateOnly | DefectState::FeeOnly)
}

/// A failing checker line counts only when `runs.txt` grew while one defect
/// remained. Lines written while both defects were still present, a stale
/// screen copy of `DUAL-HOLD`, and the test's own `run_gate` call do not.
fn new_fail_while_exactly_one(
    state: DefectState,
    fails_now: usize,
    fails_while_both: usize,
) -> bool {
    exactly_one(state) && fails_now > fails_while_both
}

/// Lines whose entire trimmed text is the checker token. A sentence that
/// only contains the token, including `Expected line: DUAL-GATE-7K`, does
/// not count.
fn pass_line_count(text: &str) -> usize {
    text.lines().filter(|line| line.trim() == PASS_LINE).count()
}

fn first_exact_offset(text: &str, token: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim() == token {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn output_prints_pass(output: &str) -> bool {
    output.lines().any(|line| line.trim() == PASS_LINE)
}

/// The quote turn must append a new exact success line. A screen quote of
/// the token does not count. `run_gate` runs in a private directory and
/// does not append the session `runs.txt`.
fn new_pass_appended(passes_now: usize, passes_before: usize) -> bool {
    passes_now > passes_before
}

/// A new `runs.txt` success line counts only after the session tool trace
/// shows `python3 gate.py` and that command's output is the success token.
/// A hand-written append and a run that sets `GATE_SKIP_LOG` do not count.
/// The user prompt contains the same words; only a bash `command` field does.
fn quote_counted(passes_now: usize, passes_before: usize, trace_delta: &str) -> bool {
    new_pass_appended(passes_now, passes_before)
        && traced_gate_exec(trace_delta)
            .is_some_and(|exec| !exec.call_id.is_empty() && exec.stdout_line == PASS_LINE)
}

/// Screen text that contains the token cannot finish the quote turn.
/// `screen_quotes` is the fresh pager line. It is necessary and not sufficient.
fn quote_turn_done(
    passes_now: usize,
    passes_before: usize,
    trace_delta: &str,
    screen_quotes: bool,
) -> bool {
    quote_counted(passes_now, passes_before, trace_delta) && screen_quotes
}

fn session_tool_trace(workspace: &Path, home: &Path) -> String {
    let mut chunks = Vec::new();
    collect_trace_files(&workspace.join(".a3s"), &mut chunks);
    collect_trace_files(&home.join(".a3s").join("logs"), &mut chunks);
    chunks.join("\n")
}

fn collect_trace_files(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_trace_files(&path, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !(name.ends_with(".jsonl") || name.ends_with(".log")) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push(text);
        }
    }
}

/// Lines appended after the quote prompt. A new log file sorts into the
/// middle of the concatenation, so a failed prefix match keeps lines the
/// earlier snapshot did not already contain.
fn trace_delta(before: &str, now: &str) -> String {
    if let Some(rest) = now.strip_prefix(before) {
        return rest.to_string();
    }
    let old: std::collections::HashSet<&str> = before.lines().collect();
    now.lines()
        .filter(|line| !line.is_empty() && !old.contains(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A bash tool call from the new trace. The call id is absent from the user
/// prompt. `None` for a hand-written append, a `GATE_SKIP_LOG` run, a stdout
/// sentence that only contains the token, and a user message that mentions
/// `python3 gate.py`.
struct GateExec {
    call_id: String,
    command: String,
    stdout_line: String,
    receipt_key: Option<String>,
    verification_effect_digest: Option<String>,
}

fn traced_gate_exec(trace_delta: &str) -> Option<GateExec> {
    let rows: Vec<serde_json::Value> = trace_delta
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if rows
        .iter()
        .any(|row| row["type"] == "tool.prepared" || row["payload"].get("receipt").is_some())
    {
        return tui_coding::typed_checker_exec(&rows);
    }
    let mut pending: Vec<(String, String)> = Vec::new();
    for line in trace_delta.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        pending.extend(turn_commands(&value));
        if let Some((id, output)) = tool_output(&value) {
            let Some(call_id) = id.filter(|call_id| !call_id.is_empty()) else {
                continue;
            };
            let Some(command) = take_command(&mut pending, Some(call_id.as_str())) else {
                continue;
            };
            if !is_live_gate_command(&command) || !output_prints_pass(&output) {
                continue;
            }
            let Some(stdout_line) = output
                .lines()
                .find(|line| line.trim() == PASS_LINE)
                .map(|line| line.trim().to_string())
            else {
                continue;
            };
            return Some(GateExec {
                call_id,
                command,
                stdout_line,
                receipt_key: None,
                verification_effect_digest: None,
            });
        }
    }
    None
}

fn traced_gate_command(trace_delta: &str) -> Option<String> {
    traced_gate_exec(trace_delta).map(|exec| exec.command)
}

fn turn_commands(value: &serde_json::Value) -> Vec<(String, String)> {
    let payload = value.get("payload").unwrap_or(value);
    if payload.get("kind").and_then(|item| item.as_str()) != Some("tool") {
        return Vec::new();
    }
    let mut found = Vec::new();
    if let Some(call) = payload.get("call") {
        push_bash_call(call, &mut found);
    }
    if let Some(also) = payload.get("also").and_then(|item| item.as_array()) {
        for call in also {
            push_bash_call(call, &mut found);
        }
    }
    found
}

fn push_bash_call(call: &serde_json::Value, found: &mut Vec<(String, String)>) {
    let name = call
        .get("name")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if !name.is_empty() && !matches!(name, "bash" | "shell" | "execute" | "Bash" | "Shell") {
        return;
    }
    let Some(command) = call
        .get("args")
        .and_then(|args| args.get("command"))
        .and_then(|item| item.as_str())
        .filter(|command| !command.is_empty())
    else {
        return;
    };
    let id = call
        .get("id")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    found.push((id, command.to_string()));
}

fn tool_output(value: &serde_json::Value) -> Option<(Option<String>, String)> {
    let kind = value
        .get("type")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    let payload = value.get("payload").unwrap_or(value);
    if kind != "tool.result" && payload.get("toolCallId").is_none() {
        return None;
    }
    let output = payload.get("output").and_then(|item| item.as_str())?;
    let id = payload
        .get("toolCallId")
        .and_then(|item| item.as_str())
        .map(str::to_string);
    Some((id, output.to_string()))
}

fn take_command(pending: &mut Vec<(String, String)>, id: Option<&str>) -> Option<String> {
    let id = id.filter(|id| !id.is_empty())?;
    let pos = pending.iter().position(|(call_id, _)| call_id == id)?;
    Some(pending.remove(pos).1)
}

fn is_live_gate_command(command: &str) -> bool {
    if command.contains("GATE_SKIP_LOG") {
        return false;
    }
    shell_segments(command)
        .iter()
        .any(|segment| segment_runs_gate(segment))
}

fn shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        if let Some(open) = quote {
            current.push(ch);
            if ch == open {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        let split = ch == ';'
            || ch == '\n'
            || (ch == '&' && chars.peek() == Some(&'&'))
            || (ch == '|' && chars.peek() != Some(&'|'));
        if ch == '|' && chars.peek() == Some(&'|') {
            chars.next();
            segments.push(std::mem::take(&mut current));
            continue;
        }
        if ch == '&' && chars.peek() == Some(&'&') {
            chars.next();
            segments.push(std::mem::take(&mut current));
            continue;
        }
        if split {
            segments.push(std::mem::take(&mut current));
            continue;
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        segments.push(current);
    }
    segments
        .into_iter()
        .filter(|segment| !segment.trim().is_empty())
        .collect()
}

fn segment_runs_gate(segment: &str) -> bool {
    let tokens = shell_tokens(segment);
    let mut index = 0;
    while index < tokens.len() && is_env_assignment(&tokens[index]) {
        index += 1;
    }
    let Some(program) = tokens.get(index) else {
        return false;
    };
    let base = program.rsplit('/').next().unwrap_or(program);
    if base == "bash" || base == "sh" {
        let Some(script) = tokens
            .iter()
            .skip(index + 1)
            .find(|token| !token.starts_with('-'))
        else {
            return false;
        };
        return is_live_gate_command(script);
    }
    if base != "python3" && base != "python" {
        return false;
    }
    if tokens.iter().skip(index + 1).any(|token| token == "-c") {
        return false;
    }
    tokens
        .iter()
        .skip(index + 1)
        .any(|token| token == "gate.py" || token.ends_with("/gate.py"))
}

fn shell_tokens(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in segment.chars() {
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn trace_fact(kind: &str, payload: serde_json::Value) -> String {
    serde_json::json!({
        "seq": 1,
        "type": kind,
        "key": "k",
        "cause": null,
        "payload": payload,
    })
    .to_string()
}

fn bash_turn(id: &str, command: &str) -> String {
    trace_fact(
        "model.turn",
        serde_json::json!({
            "kind": "tool",
            "call": {
                "id": id,
                "name": "bash",
                "args": {"command": command},
                "needs_confirmation": false
            }
        }),
    )
}

fn bash_result(id: &str, output: &str) -> String {
    trace_fact(
        "tool.result",
        serde_json::json!({
            "toolCallId": id,
            "ok": true,
            "output": output
        }),
    )
}

fn screen_is_busy(screen: &str) -> bool {
    screen.contains("Thinking") || screen.contains("[stop]")
}

fn drive_until(
    pty: &mut PtySession,
    timeout: Duration,
    mut done: impl FnMut(&str) -> bool,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = pty.screen();
        if answer_dialog(pty, &last) {
            continue;
        }
        if let Some(hit) = banner(&last) {
            panic!("turn/provider error banner {hit}\n{}", pty.redact(&last));
        }
        if done(&last) {
            std::thread::sleep(Duration::from_millis(800));
            last = pty.screen();
            if answer_dialog(pty, &last) {
                continue;
            }
            if done(&last) {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    panic!("timed out\n{}", pty.redact(&last));
}

fn names_both_defects(before: &str, screen: &str) -> bool {
    let prose = fresh_matching(before, screen, DIAG_PROMPT, is_prose);
    prose.iter().any(|line| is_prose(line))
        && (screen_has(screen, "count * 3")
            || screen_has(screen, "times 3")
            || screen_has(screen, "multiplied by 3"))
        && (screen_has(screen, "count + 11")
            || screen_has(screen, "plus 11")
            || screen_has(screen, "adds 11"))
}

#[test]
fn checker_fails_until_both_defects_are_fixed() {
    prompts_omit_secrets_and_defects();
    assert_eq!(classify(RATE_PY, FEE_PY), DefectState::Both);
    assert_eq!(predicted_line(DefectState::FeeOnly), FAIL_LINE);
    assert_eq!(predicted_line(DefectState::RateOnly), FAIL_LINE);
    assert_eq!(predicted_line(DefectState::Clear), PASS_LINE);
    let swapped_rate = RATE_PY.replace(RATE_BUG, "return count + 3");
    let swapped_fee = FEE_PY.replace(FEE_BUG, "return count * 11");
    assert_eq!(
        predicted_line(classify(&swapped_rate, &swapped_fee)),
        FAIL_LINE
    );
    assert_eq!(classify(RATE_FIXED, FEE_PY), DefectState::FeeOnly);
    assert_eq!(classify(RATE_PY, FEE_FIXED), DefectState::RateOnly);
    assert_eq!(classify(RATE_FIXED, FEE_FIXED), DefectState::Clear);
}

#[test]
fn partial_fail_requires_a_new_checker_line() {
    assert!(!new_fail_while_exactly_one(DefectState::Both, 2, 0));
    assert!(!new_fail_while_exactly_one(DefectState::FeeOnly, 2, 2));
    assert!(!new_fail_while_exactly_one(DefectState::RateOnly, 2, 2));
    assert!(new_fail_while_exactly_one(DefectState::FeeOnly, 3, 2));
    assert!(new_fail_while_exactly_one(DefectState::RateOnly, 1, 0));
    assert!(!new_fail_while_exactly_one(DefectState::Clear, 3, 1));
    assert!(!new_pass_appended(0, 0));
    assert!(!new_pass_appended(1, 1));
    assert!(new_pass_appended(1, 0));
    assert!(new_pass_appended(2, 1));
    let traced = format!(
        "{}\n{}",
        bash_turn("call-1", "python3 gate.py"),
        bash_result("call-1", "DUAL-GATE-7K\n")
    );
    assert!(quote_counted(1, 0, &traced));
    assert!(!quote_counted(1, 1, &traced));
    assert!(!quote_counted(
        1,
        0,
        "Run python3 gate.py and quote its printed line.\nDUAL-GATE-7K"
    ));
}

#[test]
fn partial_fail_quote_counts_only_a_traced_python_gate() {
    let gate = format!(
        "{}\n{}",
        bash_turn("call-1", "/usr/bin/python3 gate.py"),
        bash_result("call-1", "DUAL-GATE-7K\n")
    );
    assert_eq!(
        traced_gate_command(&gate).as_deref(),
        Some("/usr/bin/python3 gate.py")
    );
    let exec = traced_gate_exec(&gate).expect("traced python3 gate.py");
    assert_eq!(exec.call_id, "call-1");
    assert_eq!(exec.stdout_line, PASS_LINE);
    assert!(quote_counted(1, 0, &gate));
    let wrapped = format!(
        "{}\n{}",
        bash_turn("call-2", "bash -lc \"python3 gate.py\""),
        bash_result("call-2", "DUAL-GATE-7K\n")
    );
    assert!(quote_counted(1, 0, &wrapped));
    let prose = format!(
        "{}\n{}",
        bash_turn("call-prose", "python3 gate.py"),
        bash_result("call-prose", "Expected line: DUAL-GATE-7K\n")
    );
    assert!(traced_gate_command(&prose).is_none());
    assert!(!quote_counted(1, 0, &prose));
    let prefixed = format!(
        "{}\n{}",
        bash_turn("call-prefix", "python3 gate.py"),
        bash_result("call-prefix", "prefix DUAL-GATE-7K\n")
    );
    assert!(traced_gate_command(&prefixed).is_none());
    assert_eq!(pass_line_count("Expected line: DUAL-GATE-7K\n"), 0);
    assert_eq!(pass_line_count("DUAL-HOLD\nDUAL-GATE-7K\n"), 1);
    let echoed = format!(
        "{}\n{}",
        bash_turn("call-3", "echo DUAL-GATE-7K >> runs.txt"),
        bash_result("call-3", "")
    );
    assert!(traced_gate_command(&echoed).is_none());
    assert!(!quote_counted(1, 0, &echoed));
    assert!(!quote_turn_done(0, 0, "", true));
    assert!(!quote_turn_done(1, 0, &echoed, true));
    assert!(!quote_turn_done(1, 0, &prose, true));
    assert!(!quote_turn_done(1, 0, &prefixed, true));
    assert!(quote_turn_done(1, 0, &gate, true));
    assert!(!quote_turn_done(1, 0, &gate, false));
    let skipped = format!(
        "{}\n{}",
        bash_turn("call-4", "GATE_SKIP_LOG=1 python3 gate.py"),
        bash_result("call-4", "DUAL-GATE-7K\n")
    );
    assert!(!quote_counted(1, 0, &skipped));
    assert!(!quote_turn_done(1, 0, &skipped, true));
    let still_failing = format!(
        "{}\n{}",
        bash_turn("call-5", "python3 gate.py"),
        bash_result("call-5", "DUAL-HOLD\n")
    );
    assert!(!quote_counted(1, 0, &still_failing));
    let inline = format!(
        "{}\n{}",
        bash_turn(
            "call-6",
            "python3 -c \"open('runs.txt','a').write('DUAL-GATE-7K\\n')\""
        ),
        bash_result("call-6", "DUAL-GATE-7K\n")
    );
    assert!(!quote_counted(1, 0, &inline));
    let prompt = trace_fact(
        "user.message",
        serde_json::json!({"text": "Run python3 gate.py and quote its printed line."}),
    );
    assert!(!quote_counted(1, 0, &prompt));
    let earlier = format!("{gate}\n{prompt}");
    let delta = trace_delta(&earlier, &format!("{earlier}\n{echoed}"));
    assert!(!quote_counted(1, 0, &delta));
    let grew = trace_delta(&earlier, &format!("{earlier}\n{gate}"));
    assert!(quote_counted(2, 1, &grew));
    assert!(!screen_is_busy("DUAL-GATE-7K"));
    assert!(!screen_is_busy("Expected line: DUAL-GATE-7K"));
    assert!(screen_is_busy("Thinking"));
    assert!(screen_is_busy("[stop]"));
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_turns_fix_the_diagnosed_bug() {
    use tui_coding::turns::{
        assert_failed_checker, assert_verified, drive_prompt, ExpectedTerminal,
    };
    prompts_omit_secrets_and_defects();
    let mut session =
        open_live_with_verification(true).expect("A3S_CONFIG_FILE is unset; refusing to pass");
    let mut retained = tui_coding::RetainCodingTrace::new(&session, "tui-coding-multiturn");
    write_shop_fixture(&session.workspace);
    let workspace = session.workspace.clone();
    let before_diag = session.pty.screen();
    let diag = drive_prompt(&mut session, DIAG_PROMPT, ExpectedTerminal::Success);
    assert!(names_both_defects(&before_diag, &diag.screen));
    assert_eq!(read_defect_state(&workspace), DefectState::Both);
    assert_eq!(read_workspace_file(&workspace, "gate.py"), GATE_PY);
    assert!(diag
        .facts
        .iter()
        .any(|v| v["type"] == "turn.complete" && v["payload"]["terminal"] == "narrative"));
    retained.record_turn(&diag);

    let partial = drive_prompt(&mut session, PARTIAL_PROMPT, ExpectedTerminal::Unverified);
    assert_failed_checker(&partial);
    assert!(
        partial.screen.contains("Turn failed"),
        "unverified checkpoint is visible"
    );
    assert_eq!(read_defect_state(&workspace), DefectState::FeeOnly);
    assert_eq!(read_workspace_file(&workspace, "fee.py"), FEE_PY);
    assert_eq!(read_workspace_file(&workspace, "gate.py"), GATE_PY);
    assert_eq!(run_gate(&workspace), FAIL_LINE);
    let partial_runs = read_workspace_file(&workspace, "runs.txt");
    assert!(partial_runs.lines().any(|line| line == FAIL_LINE));
    assert!(!partial_runs.lines().any(|line| line == PASS_LINE));
    retained.record_turn(&partial);

    let repaired = drive_prompt(&mut session, SECOND_PROMPT, ExpectedTerminal::Success);
    assert_verified(&repaired);
    assert_eq!(read_defect_state(&workspace), DefectState::Clear);
    assert_eq!(run_gate(&workspace), PASS_LINE);
    assert_eq!(read_workspace_file(&workspace, "gate.py"), GATE_PY);
    retained.record_turn(&repaired);

    // A separate, already-settled prompt proves the final check/reply can be
    // requested again. It must not steer or join the preceding repair Run.
    let passes_before = pass_line_count(&read_workspace_file(&workspace, "runs.txt"));
    let checked = drive_prompt(&mut session, RUN_PROMPT, ExpectedTerminal::Success);
    let exec = assert_verified(&checked);
    assert!(checked.screen.contains(PASS_LINE));
    let runs = read_workspace_file(&workspace, "runs.txt");
    assert!(pass_line_count(&runs) > passes_before);
    let fail_at = first_exact_offset(&runs, FAIL_LINE).expect("failed checker log retained");
    let pass_at = first_exact_offset(&runs, PASS_LINE).expect("passing checker log retained");
    assert!(fail_at < pass_at);
    assert_eq!(run_gate(&workspace), PASS_LINE);
    assert_eq!(read_workspace_file(&workspace, "gate.py"), GATE_PY);
    assert_acl_unchanged(&session);
    retained.record_turn(&checked);
    let runs_distinct: std::collections::BTreeSet<_> = [&diag, &partial, &repaired, &checked]
        .into_iter()
        .map(|turn| &turn.run_id)
        .collect();
    assert_eq!(
        runs_distinct.len(),
        4,
        "four prompt terminals must belong to four admitted Runs"
    );
    eprintln!("ORACLE case=real-llm-turns prompts=4 distinct_runs=4 terminals=narrative,unverified,verified,verified \
        actual_partial_checker_exit=1 unknown=0 runs={} checker_call={}",
        runs.replace('\n', " | "), session.pty.redact(&exec.call_id));
    retained.finish();
}

#[test]
#[ignore = "requires the actual macOS pager/ACP and configured real provider; mandatory Layer C TUI gate"]
fn real_llm_tui_long_task_repairs_and_quotes_the_check() {
    prompts_omit_secrets_and_defects();
    let mut session =
        open_live_with_verification(true).expect("A3S_CONFIG_FILE is unset; refusing to pass");
    let mut retained = tui_coding::RetainCodingTrace::new(&session, "tui-coding-long-task");
    write_shop_fixture(&session.workspace);
    let workspace = session.workspace.clone();
    assert_eq!(read_defect_state(&workspace), DefectState::Both);
    assert_eq!(run_gate(&workspace), FAIL_LINE);
    let trace_before = session_tool_trace(&workspace, &session.home);
    let home = session.home.clone();
    let before = session.pty.screen();
    let mut saw_partial_fail = false;
    let mut fails_while_both = 0usize;
    submit_prompt(&mut session.pty, LONG_PROMPT);
    let screen = drive_until(&mut session.pty, Duration::from_secs(600), |screen| {
        let state = read_defect_state(&workspace);
        let runs_now = read_workspace_file(&workspace, "runs.txt");
        let fails_now = runs_now.matches(FAIL_LINE).count();
        if state == DefectState::Both {
            fails_while_both = fails_while_both.max(fails_now);
        }
        if new_fail_while_exactly_one(state, fails_now, fails_while_both) {
            saw_partial_fail = true;
        }
        state == DefectState::Clear
            && traced_gate_exec(&trace_delta(
                &trace_before,
                &session_tool_trace(&workspace, &home),
            ))
            .is_some()
            && fresh_matching(&before, screen, LONG_PROMPT, |line| {
                line.contains(PASS_LINE)
            })
            .into_iter()
            .any(|line| line.contains(PASS_LINE))
    });
    let runs = read_workspace_file(&workspace, "runs.txt");
    assert!(
        saw_partial_fail,
        "no new {FAIL_LINE} was appended while exactly one defect remained; fails_while_both={fails_while_both} runs={runs:?}"
    );
    assert_screen_clean(&session.pty, &screen);
    assert_eq!(run_gate(&workspace), PASS_LINE);
    assert_eq!(read_defect_state(&workspace), DefectState::Clear);
    assert_eq!(read_workspace_file(&workspace, "gate.py"), GATE_PY);
    assert_acl_unchanged(&session);
    let trace_after = session_tool_trace(&workspace, &session.home);
    let exec = traced_gate_exec(&trace_delta(&trace_before, &trace_after))
        .expect("long task must actually execute the checker through a bound typed receipt");
    tui_coding::require_verified_completion(
        &mut session,
        exec.receipt_key
            .as_deref()
            .expect("live checker must bind a current typed receipt"),
        exec.verification_effect_digest
            .as_deref()
            .expect("live checker effect digest"),
    );
    let fail_at = runs.find(FAIL_LINE);
    let pass_at = runs.find(PASS_LINE);
    let fails_before_pass = pass_at
        .map(|at| runs[..at].matches(FAIL_LINE).count())
        .unwrap_or(0);
    assert!(
        fail_at.is_some() && pass_at.is_some() && fail_at < pass_at && fails_before_pass >= 2,
        "checker runs did not fail twice before the success token: {runs:?}"
    );
    let quoted: Vec<_> = fresh_matching(&before, &screen, LONG_PROMPT, |line| {
        line.contains(PASS_LINE)
    })
    .into_iter()
    .filter(|line| line.contains(PASS_LINE))
    .collect();
    let runs_log = runs.replace('\n', " | ");
    eprintln!(
        "ORACLE case=real-llm-long runs={runs_log} partial-fix still failing fail_before_pass=true assistant={}",
        session.pty.redact(&quoted.join(" "))
    );
    retained.finish();
}

#[test]
fn marker_only_assistant_line_counts_as_the_reply() {
    let before = "\
     ❯ Remember the token TURN-TOKEN-7f3a. Reply with one short sentence and do not call any tools.   10:00 PM
     The token is TURN-TOKEN-7f3a.
     Worked for 1.1s
";
    let after = "\
     ❯ Remember the token TURN-TOKEN-7f3a. Reply with one short sentence and do not call any tools.   10:01 PM
     The token is TURN-TOKEN-7f3a.
     ❯ Reply with one short sentence that repeats the token from my first message. Do not call any tools.   10:01 PM
     ◆ Thought for 0.2s
     TURN-TOKEN-7f3a                                                                                        10:01 PM
     Worked for 2.3s
     Image in clipboard · ctrl+v to paste
";
    let lines = collect_fresh_lines(
        before,
        after,
        "Reply with one short sentence that repeats the token from my first message. Do not call any tools.",
        Some("TURN-TOKEN-7f3a"),
    );
    assert_eq!(lines, vec!["TURN-TOKEN-7f3a".to_string()]);

    let prompt_only = "\
     ❯ Remember the token TURN-TOKEN-7f3a. Reply with one short sentence and do not call any tools.   10:02 PM
";
    let leaked = collect_fresh_lines("", prompt_only, "", Some("TURN-TOKEN-7f3a"));
    assert!(
        leaked.iter().all(|line| !line.contains("TURN-TOKEN-7f3a")),
        "the user prompt row is not the assistant reply: {leaked:?}"
    );

    let sentence =
        "The token is TURN-TOKEN-7f3a.                                          10:01 PM";
    let prose = collect_fresh_lines(
        "",
        sentence,
        "Reply with one short sentence that repeats the token from my first message. Do not call any tools.",
        Some("TURN-TOKEN-7f3a"),
    );
    assert_eq!(prose, vec!["The token is TURN-TOKEN-7f3a.".to_string()]);

    let file_line = "DUAL-GATE-7K                                          10:03 PM";
    let quoted = fresh_matching("", file_line, LONG_PROMPT, |line| line.contains(PASS_LINE));
    assert_eq!(quoted, vec![PASS_LINE.to_string()]);
    assert!(!LONG_PROMPT.contains(PASS_LINE));
    assert!(
        LONG_PROMPT.len() < 100,
        "the long-task prompt must fit one composer row"
    );
}

#[test]
fn wrapped_composer_line_still_matches_the_prompt() {
    let screen = "\
  │ ❯ Read note.txt in this workspace and reply with one short sentence that quotes the token written in that file.  │
  │   Use the read tool. Do not guess.                                                                               │
";
    let prompt = "Read note.txt in this workspace and reply with one short sentence that quotes the token written in that file. Use the read tool. Do not guess.";
    assert!(!screen.contains(prompt));
    assert!(screen_has(screen, prompt));
}
