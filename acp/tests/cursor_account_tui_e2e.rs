//! PTY end-to-end: the shipped TUI, the shipped ACP agent, and a local
//! Connect stub standing in for the Cursor account endpoint.
//!
//! The catalog comes from fixture files. The turn does not spawn `agent`.

#![cfg(test)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const BEARER: &str = "cursor-fixture-bearer-9f3a7c";
const EMAIL: &str = "cursor-fixture-user@example.com";
const STUB_TEXT: &str = "fixture-e2e-pong";
const SENTINEL_BYTES: &[u8] = b"unchanged";

struct Hit {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Stub {
    origin: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

fn start_stub() -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let port = listener.local_addr().expect("addr").port();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            if let Some(hit) = read_hit(&mut stream) {
                let ok = hit.path == "/agent.v1.AgentService/Run";
                let _ = recorded.lock().map(|mut hits| hits.push(hit));
                let _ = write_stub_response(&mut stream, ok);
            }
        }
    });
    Stub {
        origin: format!("http://127.0.0.1:{port}"),
        hits,
    }
}

fn read_hit(stream: &mut TcpStream) -> Option<Hit> {
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .ok()?;
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 1024 * 1024 {
            return None;
        }
    };
    let header = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut method = String::new();
    let mut path = String::new();
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for (index, line) in header.split("\r\n").enumerate() {
        if index == 0 {
            let mut parts = line.split_whitespace();
            method = parts.next().unwrap_or("").to_string();
            path = parts.next().unwrap_or("").to_string();
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }
    let expects_continue = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("expect") && value.to_ascii_lowercase().contains("100-continue")
    });
    if expects_continue {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").ok()?;
        stream.flush().ok()?;
    }
    let mut body = buf[header_end + 4..].to_vec();
    if headers
        .iter()
        .any(|(key, value)| key.eq_ignore_ascii_case("transfer-encoding") && value == "chunked")
    {
        body = read_chunked_frame(stream, body)?;
    } else {
        while body.len() < content_length {
            let n = stream.read(&mut tmp).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(content_length);
    }
    Some(Hit {
        method,
        path,
        headers,
        body,
    })
}

fn read_chunked_frame(stream: &mut TcpStream, mut wire: Vec<u8>) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut bytes = [0u8; 4096];
    loop {
        let end = loop {
            if let Some(end) = wire.windows(2).position(|window| window == b"\r\n") {
                break end;
            }
            let count = stream.read(&mut bytes).ok()?;
            if count == 0 || wire.len() > 1024 * 1024 {
                return None;
            }
            wire.extend_from_slice(&bytes[..count]);
        };
        let size = usize::from_str_radix(
            std::str::from_utf8(&wire[..end]).ok()?.split(';').next()?,
            16,
        )
        .ok()?;
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        while wire.len() < end + 2 + size + 2 {
            let count = stream.read(&mut bytes).ok()?;
            if count == 0 {
                return None;
            }
            wire.extend_from_slice(&bytes[..count]);
        }
        body.extend_from_slice(&wire[end + 2..end + 2 + size]);
        wire.drain(..end + 2 + size + 2);
        if body.len() > 1024 * 1024 {
            return None;
        }
        if body.len() >= 5 {
            let length = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
            if body.len() >= 5 + length {
                return Some(body);
            }
        }
    }
}

fn write_stub_response(stream: &mut TcpStream, ok: bool) -> std::io::Result<()> {
    let payload = if ok {
        let text = serde_json::json!({"text":STUB_TEXT,"tool_calls":[]}).to_string();
        let mut body = envelope(
            &serde_json::json!({"interactionUpdate":{"textDelta":{"text":text}}}).to_string(),
        );
        body.extend(envelope(r#"{"interactionUpdate":{"turnEnded":{}}}"#));
        body
    } else {
        Vec::new()
    };
    let status = if ok { "200 OK" } else { "404 Not Found" };
    write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/connect+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", payload.len())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

fn envelope(json: &str) -> Vec<u8> {
    let payload = json.as_bytes();
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn envelope_json(body: &[u8]) -> serde_json::Value {
    assert!(body.len() >= 5, "connect envelope too short");
    assert_eq!(body[0], 0, "compressed connect frame");
    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    assert_eq!(body.len(), 5 + len, "request was not one connect frame");
    serde_json::from_slice(&body[5..]).expect("connect json")
}

fn json_has_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key(key) || map.values().any(|child| json_has_key(child, key))
        }
        serde_json::Value::Array(items) => items.iter().any(|child| json_has_key(child, key)),
        _ => false,
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
    marker: PathBuf,
    env: Vec<(String, String)>,
}

enum Login {
    Present,
    Disabled,
    Absent,
}

fn prepare(login: Login, endpoint: Option<&str>) -> Fixture {
    let root = tempfile::tempdir().expect("temp");
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    let cursor_home = root.path().join("cursor-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&cursor_home).unwrap();
    let sentinel = workspace.join("sentinel.txt");
    std::fs::write(&sentinel, SENTINEL_BYTES).unwrap();
    let config = home.join("config.acl");
    std::fs::write(
        &config,
        r#"
providers "mock" {
  api_key = "mock-key"
  base_url = "http://127.0.0.1:9/v1"
  models "m1" { }
}
default_model = "mock/m1"
import_accounts = ["cursor"]
"#,
    )
    .unwrap();
    std::fs::write(
        cursor_home.join("cli-config.json"),
        format!(
            r#"{{"authInfo":{{"email":"{EMAIL}","authId":"fixture-auth","userId":"fixture-user","displayName":"Fixture"}},"privacyCache":{{"ghostMode":false}}}}"#
        ),
    )
    .unwrap();
    let token = root.path().join("token");
    std::fs::write(&token, BEARER).unwrap();
    let models = root.path().join("models.txt");
    std::fs::write(
        &models,
        "Available models\n\nauto - Auto (current, default)\ncomposer-2.5 - Composer 2.5\ngpt-5.2 - GPT-5.2\n",
    )
    .unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = root.path().join("agent-invoked");
    for name in ["agent", "cursor-agent"] {
        let path = bin.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let mut path = bin.display().to_string();
    if let Some(parent) = std::env::var_os("PATH") {
        path.push(':');
        path.push_str(&parent.to_string_lossy());
    }
    let acp = env!("CARGO_BIN_EXE_a3s-code-acp");
    let a3s_home = root.path().join("a3s-home");
    std::fs::create_dir_all(&a3s_home).unwrap();
    let mut env = vec![
        ("A3S_ACP_AGENT_BIN".into(), acp.to_string()),
        ("A3S_HOME".into(), a3s_home.display().to_string()),
        ("A3S_CONFIG".into(), config.display().to_string()),
        ("A3S_DISABLE_CC_SWITCH".into(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".into(), "1".into()),
        ("HOME".into(), home.display().to_string()),
        ("TMPDIR".into(), std::env::temp_dir().display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
        ("PATH".into(), path),
    ];
    match login {
        Login::Present => {
            env.push(("A3S_CURSOR_HOME".into(), cursor_home.display().to_string()));
            env.push(("A3S_CURSOR_TOKEN_FILE".into(), token.display().to_string()));
            env.push((
                "A3S_CURSOR_MODELS_FILE".into(),
                models.display().to_string(),
            ));
        }
        Login::Disabled => {
            env.push(("A3S_DISABLE_CURSOR_ACCOUNT".into(), "1".into()));
            env.push(("A3S_CURSOR_HOME".into(), cursor_home.display().to_string()));
            env.push(("A3S_CURSOR_TOKEN_FILE".into(), token.display().to_string()));
            env.push((
                "A3S_CURSOR_MODELS_FILE".into(),
                models.display().to_string(),
            ));
        }
        Login::Absent => {}
    }
    if let Some(endpoint) = endpoint {
        env.push(("CURSOR_API_ENDPOINT".into(), endpoint.to_string()));
    }
    Fixture {
        _root: root,
        home,
        workspace,
        config,
        marker,
        env,
    }
}

fn tui_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("A3S_CODE_TUI_BIN") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/a3s-code-ui/target/debug/a3s-code-tui")
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

    /// `/quit` on an idle composer. A leftover child is killed on drop.
    fn quit(&mut self) {
        self.clear_line();
        self.escape();
        std::thread::sleep(Duration::from_millis(450));
        self.clear_line();
        self.type_text("/quit");
        self.escape();
        std::thread::sleep(Duration::from_millis(450));
        self.enter();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.drain();
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            }
        }
    }

    fn wait_for(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.screen();
            if last.contains(needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        panic!("timed out waiting for {needle:?}\n{}", redact(&last));
    }
}

fn redact(screen: &str) -> String {
    screen
        .replace(BEARER, "[redacted]")
        .replace(EMAIL, "[redacted]")
}

fn assert_screen_has_no_secret(screen: &str) {
    assert!(
        !screen.contains(BEARER) && !screen.contains(EMAIL),
        "screen showed the fixture credential\n{}",
        redact(screen)
    );
}

fn assert_workspace_untouched(fixture: &Fixture) {
    let sentinel = std::fs::read(fixture.workspace.join("sentinel.txt")).expect("sentinel");
    assert_eq!(sentinel, SENTINEL_BYTES);
    // `prepare_session_home` creates `<workspace>/.a3s/sessions` when the
    // TUI opens a session, before the model turn. That directory is the
    // product record. Anything else at the root is a turn write.
    let mut names: Vec<_> = std::fs::read_dir(&fixture.workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.retain(|name| name != ".a3s");
    assert_eq!(
        names,
        vec!["sentinel.txt".to_string()],
        "the turn wrote a file at the workspace root"
    );
    let product = fixture.workspace.join(".a3s");
    if product.is_dir() {
        let mut children: Vec<_> = std::fs::read_dir(&product)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        children.sort();
        for name in &children {
            assert!(
                name == "sessions" || name == "effect-log" || name == "memory",
                "unexpected workspace path .a3s/{name}"
            );
        }
    }
    assert!(
        !tree_contains_secret(&fixture.workspace),
        "workspace contained the fixture credential"
    );
    let acl = std::fs::read_to_string(&fixture.config).unwrap_or_default();
    assert!(
        !acl.contains(BEARER) && !acl.contains(EMAIL),
        "acl contained the fixture credential"
    );
    assert!(
        !fixture.marker.exists(),
        "agent or cursor-agent was executed"
    );
    let _ = &fixture.home;
}

fn tree_contains_secret(root: &Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes
                .windows(BEARER.len())
                .any(|window| window == BEARER.as_bytes())
                || bytes
                    .windows(EMAIL.len())
                    .any(|window| window == EMAIL.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

fn assert_cursor_run(stub: &Stub) {
    let hits = stub.hits.lock().expect("hits");
    assert!(hits.iter().all(|hit| matches!(
        hit.path.as_str(),
        "/agent.v1.AgentService/Run" | "/agent.v1.AgentService/GetUsableModels"
    )));
    let runs = hits
        .iter()
        .filter(|hit| hit.path == "/agent.v1.AgentService/Run")
        .collect::<Vec<_>>();
    assert!(!runs.is_empty(), "stub saw no model request");
    for hit in runs {
        assert_eq!(hit.method, "POST", "unexpected method {}", hit.method);
        assert_eq!(
            hit.path, "/agent.v1.AgentService/Run",
            "unexpected path {}",
            hit.path
        );
        let content_type = hit
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        assert!(
            content_type.eq_ignore_ascii_case("application/connect+json"),
            "content-type {content_type}"
        );
        let authorized = hit.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && *value == format!("Bearer {BEARER}")
        });
        assert!(authorized, "bearer mismatch");
        let value = envelope_json(&hit.body);
        assert_eq!(
            value
                .pointer("/runRequest/modelDetails/modelId")
                .and_then(serde_json::Value::as_str),
            Some("composer-2.5")
        );
        assert_eq!(
            value
                .pointer("/runRequest/action/userMessageAction/userMessage/mode")
                .and_then(serde_json::Value::as_str),
            Some("ASK")
        );
        assert!(!json_has_key(&value, "excludeWorkspaceContext"));
        assert!(!json_has_key(&value, "customSystemPrompt"));
    }
}

fn env_refs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn open_ready(fixture: &Fixture) -> PtySession {
    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "build a3s-code-tui before this test: {}",
        binary.display()
    );
    let refs = env_refs(&fixture.env);
    let mut pty = open_pty(&binary, &fixture.workspace, &refs);
    let screen = wait_ready(&mut pty, "mock/m1", Duration::from_secs(45));
    assert_screen_has_no_secret(&screen);
    assert!(
        !screen.contains("cursor ·"),
        "cursor was the default model\n{}",
        redact(&screen)
    );
    pty
}

fn wait_ready(pty: &mut PtySession, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        last = pty.screen();
        if last.contains("Do you trust the contents of this directory") {
            pty.type_text("y");
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

#[test]
fn picker_shows_the_cursor_account_and_a_turn_returns_stub_text() {
    let stub = start_stub();
    let fixture = prepare(Login::Present, Some(&stub.origin));
    let acl = std::fs::read_to_string(&fixture.config).unwrap();
    std::fs::write(
        &fixture.config,
        acl.replace(
            "default_model = \"mock/m1\"",
            "default_model = \"cursor/composer-2.5\"",
        ),
    )
    .unwrap();
    let binary = tui_binary();
    let refs = env_refs(&fixture.env);
    let mut pty = open_pty(&binary, &fixture.workspace, &refs);
    wait_ready(&mut pty, "cursor · Composer 2.5", Duration::from_secs(45));
    pty.escape();
    pty.clear_line();
    // Inspect the imported catalog without racing a session rebuild.
    pty.write_bytes(b"\x1b[200~/model \x1b[201~");
    let picker = pty.wait_for("Logged-in Cursor account", Duration::from_secs(8));
    assert!(
        picker.contains("Logged-in Cursor account"),
        "picker missed the account description\n{}",
        redact(&picker)
    );
    assert_screen_has_no_secret(&picker);
    pty.escape();
    pty.clear_line();
    let settled = pty.wait_for("cursor · Composer 2.5", Duration::from_secs(8));
    assert!(
        !settled.contains("Couldn't switch model"),
        "model switch failed\n{}",
        redact(&settled)
    );
    assert_screen_has_no_secret(&settled);
    pty.clear_line();
    pty.type_text("say the stub word");
    std::thread::sleep(Duration::from_millis(200));
    let composed = pty.screen();
    assert!(
        composed.contains("say the stub word") && composed.contains("cursor · Composer 2.5"),
        "footer did not keep the cursor model\n{}",
        redact(&composed)
    );
    assert_screen_has_no_secret(&composed);
    pty.enter();
    let answered = pty.wait_for(STUB_TEXT, Duration::from_secs(60));
    assert!(
        !answered.contains("Couldn't switch model"),
        "model switch failed\n{}",
        redact(&answered)
    );
    assert_screen_has_no_secret(&answered);
    pty.quit();
    assert_cursor_run(&stub);
    assert_workspace_untouched(&fixture);
}

#[test]
fn disable_leaves_cursor_out_and_launch_still_works() {
    let stub = start_stub();
    let fixture = prepare(Login::Disabled, Some(&stub.origin));
    let mut pty = open_ready(&fixture);
    pty.clear_line();
    pty.type_text("/model ");
    let picker = pty.wait_for("From A3S ACL provider mock", Duration::from_secs(8));
    assert_screen_has_no_secret(&picker);
    assert!(
        !picker.contains("cursor ·") && !picker.contains("Logged-in Cursor account"),
        "disabled import still offered Cursor\n{}",
        redact(&picker)
    );
    assert!(stub.hits.lock().expect("hits").is_empty());
    pty.quit();
    assert_workspace_untouched(&fixture);
}

#[test]
fn missing_login_leaves_cursor_out_and_launch_still_works() {
    let stub = start_stub();
    let fixture = prepare(Login::Absent, Some(&stub.origin));
    let mut pty = open_ready(&fixture);
    pty.clear_line();
    pty.type_text("/model ");
    let picker = pty.wait_for("From A3S ACL provider mock", Duration::from_secs(8));
    assert!(
        !picker.contains("cursor ·") && !picker.contains("Logged-in Cursor account"),
        "missing login still offered Cursor\n{}",
        redact(&picker)
    );
    assert!(stub.hits.lock().expect("hits").is_empty());
    pty.quit();
    assert_workspace_untouched(&fixture);
}

fn live_cursor_model_id() -> String {
    let output = std::process::Command::new("agent")
        .args(["models"])
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return "gpt-5.2".to_string();
    };
    if !output.status.success() {
        return "gpt-5.2".to_string();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut ids = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Available") || line.starts_with("Tip:") {
            continue;
        }
        let id = line.split(" - ").next().unwrap_or("").trim();
        if id.is_empty() || id.contains(char::is_whitespace) {
            continue;
        }
        ids.push(id.to_string());
    }
    if ids.iter().any(|id| id == "gpt-5.2") {
        return "gpt-5.2".to_string();
    }
    ids.into_iter()
        .find(|id| id != "auto" && id != "default")
        .unwrap_or_else(|| "gpt-5.2".to_string())
}

#[test]
#[ignore = "one live Cursor TUI turn; spends account quota"]
fn live_tui_turn_returns_assistant_text() {
    let home = std::env::var("HOME").expect("HOME");
    let root = tempfile::tempdir().expect("temp");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("sentinel.txt"), SENTINEL_BYTES).unwrap();
    let config = root.path().join("config.acl");
    let model_id = live_cursor_model_id();
    std::fs::write(
        &config,
        format!("import_accounts = [\"cursor\"]\ndefault_model = \"cursor/{model_id}\"\n"),
    )
    .unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    let acp = env!("CARGO_BIN_EXE_a3s-code-acp");
    let a3s_home = root.path().join("a3s-home");
    std::fs::create_dir_all(&a3s_home).unwrap();
    let env = vec![
        ("A3S_ACP_AGENT_BIN".into(), acp.to_string()),
        ("A3S_HOME".into(), a3s_home.display().to_string()),
        ("A3S_CONFIG".into(), config.display().to_string()),
        ("A3S_DISABLE_CC_SWITCH".into(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".into(), "1".into()),
        ("HOME".into(), home),
        ("TMPDIR".into(), std::env::temp_dir().display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
        ("PATH".into(), path),
    ];
    let binary = tui_binary();
    assert!(binary.is_file(), "build a3s-code-tui first");
    let refs = env_refs(&env);
    let mut pty = open_pty(&binary, &workspace, &refs);
    wait_ready(&mut pty, "cursor ·", Duration::from_secs(45));
    pty.clear_line();
    pty.type_text("Reply with the one word obtained by joining p and ong. Do not use tools.");
    pty.enter();
    let answered = pty.wait_for("pong", Duration::from_secs(90));
    assert!(
        !answered.contains("eyJ"),
        "screen looked like it contained a token"
    );
    assert_eq!(
        std::fs::read(workspace.join("sentinel.txt")).unwrap(),
        SENTINEL_BYTES
    );
}

#[tokio::test]
#[ignore = "real Cursor account read/write round trip; spends account quota"]
async fn live_cursor_tools_read_write_and_verify_actual_workspace_bytes() {
    use a3s_code_core::config::AccountImport;
    use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
    use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions};

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let marker = format!("cursor-tools-{}\n", uuid::Uuid::new_v4());
    std::fs::write(workspace.join("input.txt"), &marker).unwrap();
    let config = root.path().join("cursor.acl");
    std::fs::write(&config, "import_accounts = [\"cursor\"]\n").unwrap();
    let (launch, _) = a3s_code_acp::load_launch_config_with_imports(
        workspace.clone(),
        Some(config),
        &[AccountImport::Cursor],
    )
    .expect("local Cursor login and catalog");
    let model = "cursor/gpt-5.2";
    println!("live model {model}");
    let agent = Agent::from_config(launch.config).await.unwrap();
    let mut policy = PermissionPolicy::new().allow_all(&["read", "write"]);
    policy.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_model(model)
        .with_permission_policy(policy)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(8)
        .with_completion_attestor(Arc::new(
            a3s_code_core::completion_attestor::DiskContentAttestor::new(&workspace),
        ));
    let session = agent
        .session_async(workspace.display().to_string(), Some(options))
        .await
        .unwrap();
    let (mut events, handle) = session.stream(
        "Use only read and write. First read input.txt. Copy its exact bytes to output.txt using write, then finish. Do not run commands or delegate.",
        None,
    ).await.unwrap();
    let mut calls = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolEnd {
                    name, exit_code, ..
                } => {
                    println!("tool {name} exit {exit_code}");
                    assert_eq!(exit_code, 0, "workspace tool failed: {name}");
                    calls.push(name);
                }
                AgentEvent::End { text, .. } => return text,
                AgentEvent::Error { message } => panic!("Cursor tool round trip failed: {message}"),
                _ => {}
            }
        }
        panic!("stream ended without a final answer");
    })
    .await;
    if result.is_err() {
        handle.abort();
    }
    let _final_text = result.expect("bounded Cursor run");
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("output.txt")).unwrap(),
        marker
    );
    assert_eq!(calls, ["read", "write"]);
    println!("verified exact workspace bytes and read/write tool trace");
}
