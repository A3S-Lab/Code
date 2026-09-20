pub mod codex_login_client;
pub mod layer_c_model;

/// Executable for stdio MCP fixtures.
///
/// The Windows Store `python3` alias exits immediately and closes the MCP
/// response channel. Prefer the `py -3` launcher's real interpreter when it
/// exists; otherwise keep `python3` for Unix hosts.
pub fn python_stdio_command() -> String {
    let Ok(output) = std::process::Command::new("py")
        .args(["-3", "-c", "import sys; print(sys.executable)"])
        .output()
    else {
        return "python3".to_string();
    };
    if !output.status.success() {
        return "python3".to_string();
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let lower = path.to_ascii_lowercase();
    if path.is_empty() || lower.contains("windowsapps") || !std::path::Path::new(&path).is_file() {
        return "python3".to_string();
    }
    path
}
