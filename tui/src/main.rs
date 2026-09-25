//! `a3s code` full-screen surface. The `a3s` CLI starts this binary.

use std::env;
use std::path::PathBuf;

fn arg_value(flag: &str) -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_string());
        }
    }
    None
}

#[tokio::main]
async fn main() {
    let workspace = PathBuf::from(arg_value("--workspace").unwrap_or_else(|| ".".into()));
    let config = PathBuf::from(arg_value("--config").unwrap_or_else(|| "config.acl".into()));
    let model = arg_value("--model").unwrap_or_default();
    if let Err(error) = a3s_code_tui::run_fullscreen(&workspace, &config, &model, None).await {
        eprintln!("A3S Code TUI: {error}");
        std::process::exit(1);
    }
}
