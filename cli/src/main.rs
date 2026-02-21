//! A3S Code CLI — Interactive AI coding agent in the terminal.
//!
//! Usage:
//!   a3s-code [OPTIONS] [PROMPT]
//!
//! Examples:
//!   a3s-code                          # Interactive REPL
//!   a3s-code "Explain the auth module" # One-shot mode
//!   a3s-code -c agent.hcl "Fix bugs"  # Custom config

use a3s_code_core::{Agent, AgentEvent, CommandRegistry};
use anyhow::Result;
use clap::Parser;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// A3S Code — AI coding agent in the terminal.
#[derive(Parser, Debug)]
#[command(name = "a3s-code", version, about = "Interactive AI coding agent")]
struct Cli {
    /// Prompt to send (one-shot mode). Omit for interactive REPL.
    prompt: Option<String>,

    /// Path to config file (.hcl or .json).
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Workspace directory (default: current directory).
    #[arg(short, long, value_name = "DIR")]
    workspace: Option<PathBuf>,

    /// Model override (e.g., "openai/gpt-4o").
    #[arg(short, long)]
    model: Option<String>,

    /// Disable streaming (wait for full response).
    #[arg(long)]
    no_stream: bool,

    /// Print version and exit.
    #[arg(long)]
    version_info: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing from RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    if cli.version_info {
        println!("a3s-code {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Resolve config path
    let config_path = resolve_config(&cli.config)?;
    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // Resolve workspace
    let workspace = cli
        .workspace
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let workspace_str = workspace.display().to_string();

    // Build session options
    let mut opts = a3s_code_core::SessionOptions::default();
    if let Some(ref model) = cli.model {
        opts.model = Some(model.clone());
    }

    let session = agent.session(&workspace_str, Some(opts))?;

    // One-shot mode
    if let Some(prompt) = cli.prompt {
        if cli.no_stream {
            let result = session.send(&prompt, None).await?;
            println!("{}", result.text);
        } else {
            let (mut rx, _handle) = session.stream(&prompt, None).await?;
            while let Some(event) = rx.recv().await {
                match event {
                    AgentEvent::TextDelta { text } => {
                        print!("{text}");
                        io::stdout().flush().ok();
                    }
                    AgentEvent::End { .. } => break,
                    _ => {}
                }
            }
            println!();
        }
        return Ok(());
    }

    // Interactive REPL mode
    run_repl(&session, cli.no_stream).await
}

/// Interactive REPL loop.
async fn run_repl(session: &a3s_code_core::AgentSession, no_stream: bool) -> Result<()> {
    println!("A3S Code v{} — Interactive Mode", env!("CARGO_PKG_VERSION"));
    println!("Type /help for commands, Ctrl+C to exit.\n");

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        print!("\x1b[36m❯\x1b[0m ");
        io::stdout().flush()?;

        let mut input = String::new();
        let bytes = reader.read_line(&mut input)?;
        if bytes == 0 {
            // EOF (Ctrl+D)
            println!("\nBye!");
            break;
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Handle /quit and /exit locally
        if trimmed == "/quit" || trimmed == "/exit" {
            println!("Bye!");
            break;
        }

        // Slash commands are handled inside session.send() / session.stream()
        if no_stream || CommandRegistry::is_command(trimmed) {
            match session.send(trimmed, None).await {
                Ok(result) => println!("{}\n", result.text),
                Err(e) => eprintln!("\x1b[31mError: {e}\x1b[0m\n"),
            }
        } else {
            match session.stream(trimmed, None).await {
                Ok((mut rx, _handle)) => {
                    while let Some(event) = rx.recv().await {
                        match event {
                            AgentEvent::TextDelta { text } => {
                                print!("{text}");
                                io::stdout().flush().ok();
                            }
                            AgentEvent::ToolStart { name, .. } => {
                                print!("\x1b[33m⚡ {name}\x1b[0m ");
                                io::stdout().flush().ok();
                            }
                            AgentEvent::ToolEnd { name, .. } => {
                                println!("\x1b[32m✓ {name}\x1b[0m");
                            }
                            AgentEvent::End { .. } => break,
                            _ => {}
                        }
                    }
                    println!("\n");
                }
                Err(e) => eprintln!("\x1b[31mError: {e}\x1b[0m\n"),
            }
        }
    }

    Ok(())
}

/// Resolve config file path: explicit > A3S_CONFIG env > ~/.a3s/config.hcl > ./agent.hcl
fn resolve_config(explicit: &Option<PathBuf>) -> Result<PathBuf> {
    // 1. Explicit path
    if let Some(path) = explicit {
        if path.exists() {
            return Ok(path.clone());
        }
        anyhow::bail!("Config file not found: {}", path.display());
    }

    // 2. A3S_CONFIG env var
    if let Ok(env_path) = std::env::var("A3S_CONFIG") {
        let p = PathBuf::from(&env_path);
        if p.exists() {
            return Ok(p);
        }
    }

    // 3. ~/.a3s/config.hcl
    if let Some(home) = dirs::home_dir() {
        let home_config = home.join(".a3s").join("config.hcl");
        if home_config.exists() {
            return Ok(home_config);
        }
    }

    // 4. ./agent.hcl
    let local = PathBuf::from("agent.hcl");
    if local.exists() {
        return Ok(local);
    }

    anyhow::bail!(
        "No config file found. Provide one with -c, set A3S_CONFIG, \
         or create ~/.a3s/config.hcl or ./agent.hcl"
    )
}
