use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

fn main() -> io::Result<()> {
    let log_path = std::env::var_os("A3S_TEST_MCP_LOG").map(std::path::PathBuf::from);
    if let Some(path) = &log_path {
        append_log(
            path,
            &format!(
                "{{\"event\":\"process_started\",\"pid\":{}}}",
                std::process::id()
            ),
        )?;
    }

    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let method = string_field(&line, "method").unwrap_or_default();
        let Some(id) = integer_field(&line, "id") else {
            continue;
        };
        let result = match method.as_str() {
            "initialize" => concat!(
                "{\"protocolVersion\":\"2024-11-05\",",
                "\"capabilities\":{},",
                "\"serverInfo\":{\"name\":\"a3s-test-mcp\",\"version\":\"1\"}}"
            ),
            "tools/list" => concat!(
                "{\"tools\":[{",
                "\"name\":\"lookup\",",
                "\"description\":\"fixture\",",
                "\"inputSchema\":{\"type\":\"object\",\"additionalProperties\":false},",
                "\"annotations\":{\"readOnlyHint\":true,\"destructiveHint\":false,\"openWorldHint\":false}",
                "}]}"
            ),
            "tools/call" => "{\"content\":[{\"type\":\"text\",\"text\":\"fixture\"}],\"isError\":false}",
            _ => "null",
        };
        writeln!(
            output,
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"
        )?;
        output.flush()?;
    }

    if let Some(path) = &log_path {
        append_log(
            path,
            &format!(
                "{{\"event\":\"process_exiting\",\"pid\":{}}}",
                std::process::id()
            ),
        )?;
    }
    Ok(())
}

fn append_log(path: &Path, event: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{event}")
}

fn string_field(body: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let rest = body.split_once(&marker)?.1;
    Some(rest.split_once('"')?.0.to_string())
}

fn integer_field(body: &str, field: &str) -> Option<u64> {
    let marker = format!("\"{field}\":");
    let rest = body.split_once(&marker)?.1.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}
