#![cfg(test)]

//! Hermetic PTY cases for the shipped pager and ACP agent.
//!
//! The map of cases lives in `tui_913_inventory.md`. Nothing here calls a
//! live provider. The model is a local OpenAI-compatible server, except
//! the missing-provider row, which points at a closed port.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const ACL_MODEL: &str = "mock/gpt";
const ALT_MODEL: &str = "mock/alt";
const TURN_MARKER: &str = "TURN-MARKER-9c2e";
const ASSISTANT_TEXT: &str = "ASSISTANT-TEXT-9c2e";
const SWITCH_MARKER: &str = "AFTER-SWITCH-9c2e";
const SWITCHED_TEXT: &str = "SWITCHED-TEXT-9c2e";
const FIXTURE_KEY: &str = "hermetic-fixture-key";
const RATE_BUG: &str = "return count * 3";
const FEE_BUG: &str = "return count + 11";
const PASS_LINE: &str = "DUAL-GATE-7K";
const FAIL_LINE: &str = "DUAL-HOLD";
const DIAG_PROMPT: &str =
    "Read rate.py, fee.py, and gate.py. Name both wrong returns. Do not edit.";
const PARTIAL_PROMPT: &str = "Edit only rate.py. Run python3 gate.py and quote its line.";
const SECOND_PROMPT: &str = "Edit fee.py now. Do not run python3.";
const RUN_PROMPT: &str = "Run python3 gate.py and quote its printed line.";
const LONG_PROMPT: &str = "Run python3 gate.py first. Fix rate.py, then fee.py. Rerun after each file. Quote the final line.";
const EDIT_RATE: &str =
    r#"{"file_path":"rate.py","old_string":"return count * 3","new_string":"return count * 8"}"#;
const EDIT_FEE: &str =
    r#"{"file_path":"fee.py","old_string":"return count + 11","new_string":"return count * 5"}"#;
const RATE_PY: &str = "def sticker_price(count):\n    return count * 3\n";
const FEE_PY: &str = "def parcel_fee(count):\n    return count + 11\n";
const RATE_FIXED: &str = "def sticker_price(count):\n    return count * 8\n";
const FEE_FIXED: &str = "def parcel_fee(count):\n    return count * 5\n";
const GATE_PY: &str = "\
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
open(\"runs.txt\", \"a\", encoding=\"utf-8\").write(line + \"\\n\")\n";

/// How many of the two independent defects are still present.
///
/// `FeeOnly` means rate.py is fixed and fee.py is not. Neither a one-line
/// operator swap nor fixing just one function is `Clear`.
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

fn read_defect_state(workspace: &Path) -> DefectState {
    let rate = std::fs::read_to_string(workspace.join("rate.py")).unwrap_or_default();
    let fee = std::fs::read_to_string(workspace.join("fee.py")).unwrap_or_default();
    classify(&rate, &fee)
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
    let output = Command::new("/usr/bin/python3")
        .arg("gate.py")
        .current_dir(probe.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 gate.py");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        output.status.success() && !stdout.is_empty(),
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

#[derive(Clone)]
struct RequestNote {
    body: String,
    state: DefectState,
}

struct Mock {
    bodies: Arc<Mutex<Vec<String>>>,
    notes: Arc<Mutex<Vec<RequestNote>>>,
    workspace: Arc<Mutex<Option<PathBuf>>>,
}

fn spawn_mock() -> (SocketAddr, Mock) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let notes = Arc::new(Mutex::new(Vec::new()));
    let workspace: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let shared = Arc::clone(&bodies);
    let shared_notes = Arc::clone(&notes);
    let shared_workspace = Arc::clone(&workspace);
    std::thread::spawn(move || {
        let mut tasks = TaskCounts::default();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some(body) = read_http_body(&mut stream) {
                let response = task_reply(&body, &mut tasks).unwrap_or_else(|| reply_for(&body));
                let state = shared_workspace
                    .lock()
                    .expect("workspace")
                    .as_deref()
                    .map(read_defect_state)
                    .unwrap_or(DefectState::Both);
                {
                    let mut guard = shared.lock().expect("mock bodies");
                    guard.push(body.clone());
                }
                {
                    let mut guard = shared_notes.lock().expect("mock notes");
                    guard.push(RequestNote { body, state });
                }
                let _ = write_http(&mut stream, &response);
            }
        }
    });
    (
        addr,
        Mock {
            bodies,
            notes,
            workspace,
        },
    )
}

fn read_http_body(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 16_000_000 {
            return None;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let mut length = 0_usize;
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    while buf.len() < header_end + length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let end = (header_end + length).min(buf.len());
    Some(String::from_utf8_lossy(&buf[header_end..end]).into_owned())
}

fn write_http(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let content_type = if body.starts_with("data:") {
        "text/event-stream"
    } else {
        "application/json"
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn request_streams(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|parsed| parsed.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

#[derive(Default)]
struct TaskCounts {
    diag: u32,
    partial: u32,
    second: u32,
    run: u32,
    long: u32,
}

/// Later user prompts are matched first. A follow-up HTTP body still
/// contains every earlier turn, so the oldest phrase would swallow the rest.
/// A follow-up that lost the defect text the diagnosis discovered fails closed.
fn task_reply(body: &str, counts: &mut TaskCounts) -> Option<String> {
    let streaming = request_streams(body);
    if body.contains("Quote the final line") {
        counts.long = counts.long.saturating_add(1);
        let seq = counts.long;
        let reply = match counts.long {
            1 => reply_tool("read", r#"{"file_path":"rate.py"}"#, seq, streaming),
            2 => {
                if !body.contains(RATE_BUG) {
                    reply_text("history-missing", streaming)
                } else {
                    reply_tool("read", r#"{"file_path":"fee.py"}"#, seq, streaming)
                }
            }
            3 => {
                if !body.contains(RATE_BUG) || !body.contains(FEE_BUG) {
                    reply_text("history-missing", streaming)
                } else {
                    reply_tool("bash", r#"{"command":"python3 gate.py"}"#, seq, streaming)
                }
            }
            4 => {
                if !body.contains(FAIL_LINE) {
                    reply_text("history-missing", streaming)
                } else {
                    reply_tool("edit", EDIT_RATE, seq, streaming)
                }
            }
            5 | 6 => {
                if !body.contains(FAIL_LINE) {
                    reply_text("history-missing", streaming)
                } else if counts.long == 5 {
                    reply_tool("bash", r#"{"command":"python3 gate.py"}"#, seq, streaming)
                } else {
                    reply_tool("edit", EDIT_FEE, seq, streaming)
                }
            }
            _ => {
                if body.contains(PASS_LINE) {
                    reply_text(&format!("printed {PASS_LINE}"), streaming)
                } else if !body.contains(FAIL_LINE) {
                    reply_text("history-missing", streaming)
                } else {
                    reply_tool("bash", r#"{"command":"python3 gate.py"}"#, seq, streaming)
                }
            }
        };
        return Some(reply);
    }
    if body.contains("quote its printed line") {
        if !body.contains(FAIL_LINE) {
            return Some(reply_text("history-missing", streaming));
        }
        if body.contains(PASS_LINE) {
            return Some(reply_text(&format!("printed {PASS_LINE}"), streaming));
        }
        counts.run = counts.run.saturating_add(1);
        return Some(reply_tool(
            "bash",
            r#"{"command":"python3 gate.py"}"#,
            counts.run,
            streaming,
        ));
    }
    if body.contains("Edit fee.py now") {
        if !body.contains(FEE_BUG) || !body.contains(FAIL_LINE) {
            return Some(reply_text("history-missing", streaming));
        }
        counts.second = counts.second.saturating_add(1);
        if counts.second == 1 {
            return Some(reply_tool("edit", EDIT_FEE, counts.second, streaming));
        }
        return Some(reply_text("fee.py now returns count * 5.", streaming));
    }
    if body.contains("Edit only rate.py") {
        if !body.contains(RATE_BUG) || !body.contains(FEE_BUG) {
            return Some(reply_text("history-missing", streaming));
        }
        counts.partial = counts.partial.saturating_add(1);
        let seq = counts.partial;
        let reply = match counts.partial {
            1 => reply_tool("edit", EDIT_RATE, seq, streaming),
            2 => reply_tool("bash", r#"{"command":"python3 gate.py"}"#, seq, streaming),
            _ => {
                if body.contains(FAIL_LINE) {
                    reply_text(&format!("checker printed {FAIL_LINE}"), streaming)
                } else {
                    reply_tool("bash", r#"{"command":"python3 gate.py"}"#, seq, streaming)
                }
            }
        };
        return Some(reply);
    }
    if body.contains("Name both wrong returns") {
        counts.diag = counts.diag.saturating_add(1);
        if body.contains(RATE_BUG) && body.contains(FEE_BUG) {
            return Some(reply_text(
                "sticker_price returns count * 3. parcel_fee returns count + 11.",
                streaming,
            ));
        }
        let file = if body.contains(RATE_BUG) {
            "fee.py"
        } else {
            "rate.py"
        };
        return Some(reply_tool(
            "read",
            &format!(r#"{{"file_path":"{file}"}}"#),
            counts.diag,
            streaming,
        ));
    }
    None
}

fn reply_for(raw: &str) -> String {
    let streaming = request_streams(raw);
    let text = if raw.contains(TURN_MARKER) {
        ASSISTANT_TEXT
    } else if raw.contains(SWITCH_MARKER) {
        SWITCHED_TEXT
    } else {
        "ok"
    };
    reply_text(text, streaming)
}

fn answer_tool_prompt(pty: &mut PtySession, screen: &str) -> bool {
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

fn wait_pumped(pty: &mut PtySession, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = pty.screen();
        if answer_tool_prompt(pty, &last) {
            continue;
        }
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("timed out waiting for {needle:?}\n{}", redact(&last));
}

fn reply_tool(name: &str, args: &str, seq: u32, streaming: bool) -> String {
    let id = format!("call_{name}_{seq}");
    if !streaming {
        return serde_json::json!({
            "id": "cmpl",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": args }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string();
    }
    let chunk = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "function": { "name": name, "arguments": args }
                }]
            }
        }]
    });
    let done = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn reply_text(text: &str, streaming: bool) -> String {
    if !streaming {
        return serde_json::json!({
            "id": "cmpl",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop"
            }]
        })
        .to_string();
    }
    let chunk = serde_json::json!({
        "choices": [{ "delta": { "content": text } }]
    });
    let done = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn request_model(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

fn tui_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("A3S_CODE_TUI_BIN") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/a3s-code-ui/target/debug/a3s-code-tui")
}

fn require_tui() -> PathBuf {
    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "build a3s-code-tui before this test: {}",
        binary.display()
    );
    binary
}

struct PtySession {
    parser: vt100::Parser,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    raw: Vec<u8>,
    cpr_replied_through: usize,
}

fn open_pty(binary: &Path, cwd: &Path, env: &[(&str, &str)]) -> PtySession {
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
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl PtySession {
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
            panic!("pty write failed: {error}\n{}", redact(&self.screen()));
        }
        self.writer.flush().ok();
    }

    fn type_text(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    fn enter(&mut self) {
        self.write_bytes(b"\r");
    }

    fn escape(&mut self) {
        self.write_bytes(b"\x1b");
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
            if last.contains(needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        panic!("timed out waiting for {needle:?}\n{}", redact(&last));
    }
}

fn redact(screen: &str) -> String {
    screen.replace(FIXTURE_KEY, "[redacted]")
}

fn assert_no_secret(screen: &str) {
    assert!(
        !screen.contains(FIXTURE_KEY),
        "screen showed the fixture key\n{}",
        redact(screen)
    );
}

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    env: Vec<(String, String)>,
}

fn write_acl(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("acl dir");
    }
    std::fs::write(path, body).expect("acl");
}

fn child_env(config: &Path, home: &Path) -> Vec<(String, String)> {
    let acp = env!("CARGO_BIN_EXE_a3s-code-acp");
    vec![
        ("A3S_ACP_AGENT_BIN".into(), acp.to_string()),
        ("A3S_CONFIG".into(), config.display().to_string()),
        ("A3S_DISABLE_CC_SWITCH".into(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".into(), "1".into()),
        ("A3S_DISABLE_CURSOR_ACCOUNT".into(), "1".into()),
        ("HOME".into(), home.display().to_string()),
        ("GROK_HOME".into(), home.join("grok").display().to_string()),
        ("TMPDIR".into(), std::env::temp_dir().display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
        (
            "PATH".into(),
            "/usr/bin:/bin:/usr/sbin:/opt/homebrew/bin".into(),
        ),
    ]
}

fn prepare(acl_body: &str) -> Fixture {
    let root = tempfile::tempdir().expect("temp");
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(home.join("grok")).expect("grok home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("sentinel.txt"), b"unchanged").expect("sentinel");
    let config = workspace.join(".a3s").join("config.acl");
    write_acl(&config, acl_body);
    let env = child_env(&config, &home);
    Fixture {
        _root: root,
        workspace,
        env,
    }
}

fn catalog_acl(base_url: &str) -> String {
    format!(
        r#"
providers "mock" {{
  base_url = "{base_url}"
  api_key = "{FIXTURE_KEY}"
  models "gpt" {{ }}
  models "alt" {{ }}
}}
default_model = "{ACL_MODEL}"
"#
    )
}

fn env_refs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn open_on_model(fixture: &Fixture, model: &str) -> PtySession {
    let binary = require_tui();
    let refs = env_refs(&fixture.env);
    let mut pty = open_pty(&binary, &fixture.workspace, &refs);
    let screen = pty.wait_for(model, Duration::from_secs(45));
    assert_no_secret(&screen);
    pty
}

fn submit_line(pty: &mut PtySession, text: &str) {
    pty.clear_line();
    pty.type_text(text);
    let typed = pty.wait_for(text, Duration::from_secs(3));
    assert_no_secret(&typed);
    pty.enter();
}

/// A catalog row still opens the effort menu. Escape leaves the typed id.
fn submit_model(pty: &mut PtySession, command: &str) {
    pty.clear_line();
    pty.type_text(command);
    let typed = pty.wait_for(command, Duration::from_secs(3));
    assert_no_secret(&typed);
    pty.escape();
    std::thread::sleep(Duration::from_millis(450));
    let armed = pty.screen();
    assert!(
        armed.contains(command),
        "escape cleared the model command\n{}",
        redact(&armed)
    );
    assert_no_secret(&armed);
    pty.enter();
}

fn wait_for_refusal(pty: &mut PtySession, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = pty.screen();
        let lower = last.to_ascii_lowercase();
        if lower.contains("connection refused")
            || last.contains("Connection failed")
            || last.contains("Request failed")
        {
            return last;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!(
        "timed out waiting for a provider refusal\n{}",
        redact(&last)
    );
}

fn version_line(binary: &Path) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .expect("version");
    assert!(
        output.status.success(),
        "version status {:?}",
        output.status
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn shipped_binaries_report_current_version() {
    let tui = version_line(&require_tui());
    let acp = version_line(&PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp")));
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        tui.split_whitespace().any(|part| part == version),
        "pager version {tui}"
    );
    assert_eq!(acp, version, "acp version {acp}");
    eprintln!("VERSION tui={tui}");
    eprintln!("VERSION acp={acp}");
}

#[test]
fn composer_opens_on_the_acl_default_model() {
    let (addr, _mock) = spawn_mock();
    let fixture = prepare(&catalog_acl(&format!("http://{addr}/v1")));
    let mut pty = open_on_model(&fixture, ACL_MODEL);
    let screen = pty.screen();
    assert!(
        screen.contains(ACL_MODEL),
        "composer missed the ACL default\n{}",
        redact(&screen)
    );
    assert_no_secret(&screen);
    let sentinel = std::fs::read(fixture.workspace.join("sentinel.txt")).expect("sentinel");
    assert_eq!(sentinel, b"unchanged");
    eprintln!("ORACLE case=launch model={ACL_MODEL}");
}

#[test]
fn one_turn_shows_assistant_text() {
    let (addr, mock) = spawn_mock();
    let fixture = prepare(&catalog_acl(&format!("http://{addr}/v1")));
    let mut pty = open_on_model(&fixture, ACL_MODEL);
    submit_line(&mut pty, TURN_MARKER);
    let answered = pty.wait_for(ASSISTANT_TEXT, Duration::from_secs(60));
    assert_no_secret(&answered);
    assert!(
        !answered.contains("Connection failed") && !answered.contains("Request failed"),
        "turn showed a provider error\n{}",
        redact(&answered)
    );
    let bodies = mock.bodies.lock().expect("bodies").clone();
    assert!(
        bodies.iter().any(|body| body.contains(TURN_MARKER)),
        "the turn never reached the mock"
    );
    eprintln!("ORACLE case=turn text={ASSISTANT_TEXT}");
}

#[test]
fn model_command_selects_the_catalog_model() {
    let (addr, mock) = spawn_mock();
    let fixture = prepare(&catalog_acl(&format!("http://{addr}/v1")));
    let mut pty = open_on_model(&fixture, ACL_MODEL);
    submit_model(&mut pty, &format!("/model {ALT_MODEL}"));
    let settled = pty.wait_for(ALT_MODEL, Duration::from_secs(15));
    assert!(
        !settled.contains("Couldn't switch model"),
        "model switch failed\n{}",
        redact(&settled)
    );
    assert_no_secret(&settled);
    submit_line(&mut pty, SWITCH_MARKER);
    let answered = pty.wait_for(SWITCHED_TEXT, Duration::from_secs(60));
    assert!(
        !answered.contains("Couldn't switch model"),
        "model switch failed\n{}",
        redact(&answered)
    );
    assert_no_secret(&answered);
    let bodies = mock.bodies.lock().expect("bodies").clone();
    let switched: Vec<_> = bodies
        .iter()
        .filter(|body| body.contains(SWITCH_MARKER))
        .map(|body| request_model(body).unwrap_or_default())
        .collect();
    assert!(
        !switched.is_empty(),
        "the follow-up turn never reached the mock"
    );
    assert!(
        switched
            .iter()
            .all(|model| model == "alt" || model.ends_with("/alt")),
        "follow-up turn did not use {ALT_MODEL}: {switched:?}"
    );
    eprintln!("ORACLE case=model selected={ALT_MODEL}");
}

#[test]
fn missing_provider_refuses_without_an_invented_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("closed port");
    let port = listener.local_addr().expect("port").port();
    drop(listener);
    let fixture = prepare(&format!(
        r#"
providers "missing" {{
  base_url = "http://127.0.0.1:{port}/v1"
  api_key = "{FIXTURE_KEY}"
  models "none" {{ }}
}}
default_model = "missing/none"
"#
    ));
    let mut pty = open_on_model(&fixture, "missing/none");
    submit_line(&mut pty, "MISSING-PROVIDER-9c2e");
    let refused = wait_for_refusal(&mut pty, Duration::from_secs(30));
    assert_no_secret(&refused);
    assert!(
        refused.contains("MISSING-PROVIDER-9c2e"),
        "the prompt was not submitted\n{}",
        redact(&refused)
    );
    assert!(
        !refused.contains(ASSISTANT_TEXT) && !refused.contains(SWITCHED_TEXT),
        "a missing provider invented a reply\n{}",
        redact(&refused)
    );
    let sentinel = std::fs::read(fixture.workspace.join("sentinel.txt")).expect("sentinel");
    assert_eq!(sentinel, b"unchanged");
    eprintln!("ORACLE case=missing-provider refusal=Connection failed");
}

fn arm_shop(mock: &Mock, workspace: &Path) {
    write_shop_fixture(workspace);
    *mock.workspace.lock().expect("workspace") = Some(workspace.to_path_buf());
}

fn gate_bytes(workspace: &Path) -> String {
    std::fs::read_to_string(workspace.join("gate.py")).expect("gate.py")
}

#[test]
fn checker_fails_until_both_defects_are_fixed() {
    prompts_omit_secrets_and_defects();
    assert_eq!(classify(RATE_PY, FEE_PY), DefectState::Both);
    assert_eq!(predicted_line(DefectState::Both), FAIL_LINE);
    assert_eq!(classify(RATE_FIXED, FEE_PY), DefectState::FeeOnly);
    assert_eq!(predicted_line(DefectState::FeeOnly), FAIL_LINE);
    assert_eq!(classify(RATE_PY, FEE_FIXED), DefectState::RateOnly);
    assert_eq!(predicted_line(DefectState::RateOnly), FAIL_LINE);
    assert_eq!(classify(RATE_FIXED, FEE_FIXED), DefectState::Clear);
    assert_eq!(predicted_line(DefectState::Clear), PASS_LINE);

    let swapped_rate = RATE_PY.replace(RATE_BUG, "return count + 3");
    let swapped_fee = FEE_PY.replace(FEE_BUG, "return count * 11");
    assert_eq!(classify(&swapped_rate, FEE_PY), DefectState::Both);
    assert_eq!(classify(RATE_PY, &swapped_fee), DefectState::Both);
    assert_eq!(
        predicted_line(classify(&swapped_rate, &swapped_fee)),
        FAIL_LINE
    );
    assert!(!has_factor("return count * 50", 5));
    assert!(!has_factor("return count * 80", 8));
    assert!(has_factor("return 8 * count", 8));
    assert!(price_fixed(
        "def sticker_price(count):\n    return 8 * count\n"
    ));

    let root = tempfile::tempdir().expect("temp");
    let cases = [
        (RATE_PY, FEE_PY, FAIL_LINE),
        (RATE_FIXED, FEE_PY, FAIL_LINE),
        (RATE_PY, FEE_FIXED, FAIL_LINE),
        (RATE_FIXED, FEE_FIXED, PASS_LINE),
        (swapped_rate.as_str(), FEE_PY, FAIL_LINE),
        (RATE_PY, swapped_fee.as_str(), FAIL_LINE),
    ];
    for (rate, fee, expect) in cases {
        std::fs::write(root.path().join("rate.py"), rate).expect("rate");
        std::fs::write(root.path().join("fee.py"), fee).expect("fee");
        std::fs::write(root.path().join("gate.py"), GATE_PY).expect("gate");
        assert_eq!(
            predicted_line(classify(rate, fee)),
            expect,
            "helper disagreed with the case"
        );
        assert_eq!(
            run_gate(root.path()),
            expect,
            "checker disagreed with the helper"
        );
    }
    assert!(
        !root.path().join("runs.txt").exists(),
        "the test checker wrote the session runs.txt"
    );
    std::fs::write(root.path().join("rate.py"), RATE_PY).expect("rate");
    std::fs::write(root.path().join("fee.py"), FEE_PY).expect("fee");
    for _ in 0..2 {
        let logged = Command::new("/usr/bin/python3")
            .arg("gate.py")
            .current_dir(root.path())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("logged gate");
        assert_eq!(
            String::from_utf8_lossy(&logged.stdout).trim(),
            FAIL_LINE,
            "stderr={}",
            String::from_utf8_lossy(&logged.stderr)
        );
    }
    let runs = std::fs::read_to_string(root.path().join("runs.txt")).expect("runs.txt");
    assert_eq!(runs, format!("{FAIL_LINE}\n{FAIL_LINE}\n"));
}

#[test]
fn three_turns_fix_the_bug_they_diagnosed() {
    prompts_omit_secrets_and_defects();
    let (addr, mock) = spawn_mock();
    let fixture = prepare(&catalog_acl(&format!("http://{addr}/v1")));
    arm_shop(&mock, &fixture.workspace);
    let mut pty = open_on_model(&fixture, ACL_MODEL);
    submit_line(&mut pty, DIAG_PROMPT);
    let first = wait_pumped(&mut pty, "returns count * 3", Duration::from_secs(90));
    assert_no_secret(&first);
    assert!(
        first.contains("returns count + 11"),
        "diagnosis did not name both defects\n{}",
        redact(&first)
    );
    assert_eq!(read_defect_state(&fixture.workspace), DefectState::Both);
    assert_eq!(gate_bytes(&fixture.workspace), GATE_PY);

    submit_line(&mut pty, PARTIAL_PROMPT);
    let second = wait_pumped(
        &mut pty,
        "checker printed DUAL-HOLD",
        Duration::from_secs(90),
    );
    assert_no_secret(&second);
    assert!(
        !second.contains("history-missing"),
        "the partial turn lost a discovered defect\n{}",
        redact(&second)
    );
    assert_eq!(read_defect_state(&fixture.workspace), DefectState::FeeOnly);
    assert_eq!(run_gate(&fixture.workspace), FAIL_LINE);
    assert_eq!(predicted_line(DefectState::FeeOnly), FAIL_LINE);
    assert_eq!(gate_bytes(&fixture.workspace), GATE_PY);
    assert!(
        !second.contains(PASS_LINE),
        "the partial fix printed the success token\n{}",
        redact(&second)
    );

    submit_line(&mut pty, SECOND_PROMPT);
    let third = wait_pumped(&mut pty, "now returns count * 5", Duration::from_secs(90));
    assert_no_secret(&third);
    assert!(
        !third.contains("history-missing"),
        "the second fix lost history"
    );
    assert_eq!(read_defect_state(&fixture.workspace), DefectState::Clear);
    assert_eq!(gate_bytes(&fixture.workspace), GATE_PY);
    assert!(
        !third.contains(PASS_LINE),
        "the success token appeared before the checker was quoted\n{}",
        redact(&third)
    );

    submit_line(&mut pty, RUN_PROMPT);
    let fourth = wait_pumped(
        &mut pty,
        &format!("printed {PASS_LINE}"),
        Duration::from_secs(90),
    );
    assert_no_secret(&fourth);
    assert!(
        !fourth.contains("history-missing") && !fourth.contains("Connection failed"),
        "the quote turn failed\n{}",
        redact(&fourth)
    );
    assert_eq!(run_gate(&fixture.workspace), PASS_LINE);
    assert_eq!(gate_bytes(&fixture.workspace), GATE_PY);
    let notes = mock.notes.lock().expect("notes").clone();
    let fail_at = notes.iter().position(|note| note.body.contains(FAIL_LINE));
    let pass_at = notes.iter().position(|note| note.body.contains(PASS_LINE));
    assert!(
        fail_at.is_some() && pass_at.is_some() && fail_at < pass_at,
        "the failing checker output was not in a model request before the success token"
    );
    assert!(
        notes.iter().any(|note| {
            note.state == DefectState::FeeOnly
                && note.body.contains(FAIL_LINE)
                && note.body.contains("Edit only rate.py")
        }),
        "no request made while one defect remained carried the failing output"
    );
    eprintln!(
        "ORACLE case=turns token={PASS_LINE} partial-fix still failing fail_before_pass=true states=both,one,none"
    );
}

#[test]
fn long_task_repairs_the_check_after_a_failure() {
    prompts_omit_secrets_and_defects();
    let (addr, mock) = spawn_mock();
    let fixture = prepare(&catalog_acl(&format!("http://{addr}/v1")));
    arm_shop(&mock, &fixture.workspace);
    let mut pty = open_on_model(&fixture, ACL_MODEL);
    submit_line(&mut pty, LONG_PROMPT);
    let screen = wait_pumped(
        &mut pty,
        &format!("printed {PASS_LINE}"),
        Duration::from_secs(120),
    );
    assert_no_secret(&screen);
    assert!(
        !screen.contains("Connection failed")
            && !screen.contains("Request failed")
            && !screen.contains("history-missing"),
        "long task failed\n{}",
        redact(&screen)
    );
    assert_eq!(read_defect_state(&fixture.workspace), DefectState::Clear);
    assert_eq!(run_gate(&fixture.workspace), PASS_LINE);
    assert_eq!(gate_bytes(&fixture.workspace), GATE_PY);
    let runs = std::fs::read_to_string(fixture.workspace.join("runs.txt")).unwrap_or_default();
    let fail_at = runs.find(FAIL_LINE);
    let pass_at = runs.find(PASS_LINE);
    assert!(
        fail_at.is_some()
            && pass_at.is_some()
            && fail_at < pass_at
            && runs[..pass_at.unwrap()].matches(FAIL_LINE).count() >= 2,
        "checker log did not fail twice before the pass: {runs:?}"
    );
    let notes = mock.notes.lock().expect("notes").clone();
    let repair: Vec<_> = notes
        .iter()
        .filter(|note| note.body.contains("Quote the final line"))
        .collect();
    let fail_at = repair.iter().position(|note| note.body.contains(FAIL_LINE));
    let pass_at = repair.iter().position(|note| note.body.contains(PASS_LINE));
    let partial = repair
        .iter()
        .any(|note| note.state == DefectState::FeeOnly && note.body.contains(FAIL_LINE));
    assert!(
        repair.len() >= 6 && fail_at.is_some() && pass_at.is_some() && fail_at < pass_at && partial,
        "long task missed partial failure or fail-before-pass: {} requests",
        repair.len()
    );
    eprintln!(
        "ORACLE case=long-task marker={PASS_LINE} requests={} partial-fix still failing fail_before_pass=true",
        repair.len()
    );
}
