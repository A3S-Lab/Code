//! Hybrid scaffold binary. `a3s code` starts the pager `a3s-code-tui`, not
//! this process. The CLI flags are `--cwd`, `--model`, `--resume <id>`, and
//! `--continue` (CLI issue #149: the CLI passes them; this binary parses
//! them). `--workspace` remains accepted as a `--cwd` alias.

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
    if env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("a3s-code-tui-hybrid {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let workspace = PathBuf::from(
        arg_value("--cwd")
            .or_else(|| arg_value("--workspace"))
            .unwrap_or_else(|| ".".into()),
    );
    let explicit = arg_value("--explicit-config")
        .or_else(|| arg_value("--config"))
        .map(PathBuf::from);
    let home = arg_value("--home").map(PathBuf::from);
    let model = arg_value("--model").unwrap_or_default();
    // `--resume <id>` targets that fact-log thread; `--continue` is the
    // scaffold default by construction (the default thread IS the
    // continuation), parsed here so it is never a dropped arg.
    let resume = arg_value("--resume");
    let continue_flag = env::args().skip(1).any(|arg| arg == "--continue");
    let _ = continue_flag;
    if let Err(error) = a3s_code_tui::run_fullscreen(
        &workspace,
        explicit.as_deref(),
        home.as_deref(),
        &model,
        None,
        resume,
    )
    .await
    {
        eprintln!("A3S Code TUI: {error}");
        std::process::exit(1);
    }
}
