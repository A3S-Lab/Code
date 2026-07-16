use std::{
    fs::OpenOptions,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
};

fn main() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let log_path = executable.with_extension("log");
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    append_log(
        &log_path,
        &format!(
            "{{\"event\":\"process_started\",\"pid\":{}}}",
            std::process::id()
        ),
    )?;
    let push_diagnostics = executable_name.contains("push-diagnostics");
    let ignore_shutdown = executable_name.contains("ignore-shutdown");
    let cold_navigation = if executable_name.contains("cold-empty") {
        ColdNavigation::Empty
    } else if executable_name.contains("cold-partial") {
        ColdNavigation::Partial
    } else {
        ColdNavigation::Disabled
    };
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut document_uri = None;
    let mut navigation_requests = 0_usize;

    while let Some(body) = read_message(&mut input)? {
        append_log(&log_path, &body)?;
        let method = string_field(&body, "method").unwrap_or_default();
        if executable_name.contains("slow-initialize") && method == "initialize" {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if method == "workspace/symbol" && body.contains("\"query\":\"terminate-process\"") {
            eprintln!("fixture language server terminated unexpectedly");
            std::process::exit(12);
        }
        if method == "textDocument/didOpen" {
            document_uri = string_field(&body, "uri");
            if push_diagnostics {
                if let Some(uri) = document_uri.as_deref() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    write_message(&mut output, &publish_diagnostics_notification(uri))?;
                }
            }
        }
        if ignore_shutdown && matches!(method.as_str(), "shutdown" | "exit") {
            continue;
        }

        let Some(id) = request_id(&body) else {
            if method == "exit" {
                break;
            }
            continue;
        };
        if is_navigation_method(&method) {
            navigation_requests += 1;
        }
        let result = response_for(
            &method,
            document_uri.as_deref(),
            push_diagnostics,
            cold_navigation,
            navigation_requests,
        );
        write_message(
            &mut output,
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"),
        )?;
    }
    append_log(
        &log_path,
        &format!(
            "{{\"event\":\"process_exiting\",\"pid\":{}}}",
            std::process::id()
        ),
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ColdNavigation {
    Disabled,
    Empty,
    Partial,
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_message(writer: &mut impl Write, body: &str) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}

fn append_log(path: &PathBuf, body: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{body}")
}

fn request_id(body: &str) -> Option<&str> {
    let rest = body.split_once("\"id\":")?.1;
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn string_field(body: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let rest = body.split_once(&marker)?.1;
    let mut escaped = false;
    let end = rest.char_indices().find_map(|(index, character)| {
        if character == '"' && !escaped {
            return Some(index);
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
        None
    })?;
    Some(rest[..end].to_owned())
}

fn response_for(
    method: &str,
    uri: Option<&str>,
    push_diagnostics: bool,
    cold_navigation: ColdNavigation,
    navigation_requests: usize,
) -> String {
    let uri = uri.unwrap_or("file:///missing.rs");
    match method {
        "initialize" => initialize_response(push_diagnostics),
        "textDocument/documentSymbol" => concat!(
            "[{\"name\":\"answer\",\"kind\":12,",
            "\"range\":{\"start\":{\"line\":0,\"character\":0},",
            "\"end\":{\"line\":0,\"character\":30}},",
            "\"selectionRange\":{\"start\":{\"line\":0,\"character\":7},",
            "\"end\":{\"line\":0,\"character\":13}}}]"
        )
        .to_owned(),
        "workspace/symbol" => workspace_symbol_response(uri),
        "textDocument/definition"
        | "textDocument/declaration"
        | "textDocument/references"
        | "textDocument/implementation" => match (cold_navigation, navigation_requests) {
            (ColdNavigation::Empty, 1) => "[]".to_owned(),
            (ColdNavigation::Partial, 1) => locations_response(uri),
            (ColdNavigation::Empty | ColdNavigation::Partial, _) => settled_locations_response(uri),
            (ColdNavigation::Disabled, _) => locations_response(uri),
        },
        "textDocument/diagnostic" => concat!(
            "{\"kind\":\"full\",\"resultId\":\"fixture-1\",\"items\":[{",
            "\"range\":{\"start\":{\"line\":0,\"character\":7},",
            "\"end\":{\"line\":0,\"character\":13}},",
            "\"severity\":2,\"source\":\"fixture\",\"message\":\"fixture warning\"}]}"
        )
        .to_owned(),
        "shutdown" => "null".to_owned(),
        _ => "null".to_owned(),
    }
}

fn is_navigation_method(method: &str) -> bool {
    matches!(
        method,
        "textDocument/definition"
            | "textDocument/declaration"
            | "textDocument/references"
            | "textDocument/implementation"
    )
}

fn initialize_response(push_diagnostics: bool) -> String {
    let mut response = concat!(
        "{\"capabilities\":{",
        "\"positionEncoding\":\"utf-16\",",
        "\"textDocumentSync\":{\"openClose\":true,\"change\":1,\"save\":true},",
        "\"documentSymbolProvider\":true,\"workspaceSymbolProvider\":true,",
        "\"definitionProvider\":true,\"declarationProvider\":true,",
        "\"referencesProvider\":true,\"implementationProvider\":true"
    )
    .to_owned();
    if !push_diagnostics {
        response.push_str(concat!(
            ",\"diagnosticProvider\":{\"interFileDependencies\":false,",
            "\"workspaceDiagnostics\":false}"
        ));
    }
    response.push_str("}}");
    response
}

fn publish_diagnostics_notification(uri: &str) -> String {
    let mut notification = concat!(
        "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",",
        "\"params\":{\"uri\":\""
    )
    .to_owned();
    notification.push_str(uri);
    notification.push_str(concat!(
        "\",\"version\":1,\"diagnostics\":[{",
        "\"range\":{\"start\":{\"line\":0,\"character\":7},",
        "\"end\":{\"line\":0,\"character\":13}},",
        "\"severity\":2,\"source\":\"fixture\",",
        "\"message\":\"fixture push warning\"}]}}"
    ));
    notification
}

fn workspace_symbol_response(uri: &str) -> String {
    let mut response = "[{\"name\":\"answer\",\"kind\":12,\"location\":{\"uri\":\"".to_owned();
    response.push_str(uri);
    response.push_str(concat!(
        "\",\"range\":{\"start\":{\"line\":0,\"character\":7},",
        "\"end\":{\"line\":0,\"character\":13}}}}]"
    ));
    response
}

fn locations_response(uri: &str) -> String {
    let mut response = "[{\"uri\":\"".to_owned();
    response.push_str(uri);
    response.push_str(concat!(
        "\",\"range\":{\"start\":{\"line\":0,\"character\":7},",
        "\"end\":{\"line\":0,\"character\":13}}}]"
    ));
    response
}

fn settled_locations_response(uri: &str) -> String {
    let mut response = String::from("[");
    for (index, character) in [0, 7, 14].into_iter().enumerate() {
        if index > 0 {
            response.push(',');
        }
        response.push_str("{\"uri\":\"");
        response.push_str(uri);
        response.push_str(&format!(
            "\",\"range\":{{\"start\":{{\"line\":0,\"character\":{character}}},\"end\":{{\"line\":0,\"character\":{}}}}}}}",
            character + 1
        ));
    }
    response.push(']');
    response
}
