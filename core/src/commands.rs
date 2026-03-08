//! Slash Commands — Interactive session commands
//!
//! Provides a `/command` system for interactive sessions. Commands are
//! dispatched before the LLM — if input starts with `/`, it's handled
//! by the command registry instead of being sent to the model.
//!
//! ## Built-in Commands
//!
//! | Command | Description |
//! |---------|-------------|
//! | `/help` | List available commands |
//! | `/compact` | Manually trigger context compaction |
//! | `/cost` | Show token usage and estimated cost |
//! | `/model` | Show or switch the current model |
//! | `/clear` | Clear conversation history |
//! | `/history` | Show conversation turn count and token stats |
//! | `/tools` | List registered tools |
//! | `/mcp` | List connected MCP servers and their tools |
//! | `/loop` | Schedule a recurring prompt |
//! | `/cron-list` | List all scheduled recurring prompts |
//! | `/cron-cancel` | Cancel a scheduled task by ID |
//!
//! ## Custom Commands
//!
//! ```rust,no_run
//! use a3s_code_core::commands::{SlashCommand, CommandContext, CommandOutput};
//!
//! struct MyCommand;
//!
//! impl SlashCommand for MyCommand {
//!     fn name(&self) -> &str { "greet" }
//!     fn description(&self) -> &str { "Say hello" }
//!     fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandOutput {
//!         CommandOutput::text("Hello from custom command!")
//!     }
//! }
//! ```

use crate::scheduler::{format_duration, parse_loop_args, CronScheduler};
use std::collections::HashMap;
use std::sync::Arc;

/// Context passed to every slash command execution.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// Current session ID.
    pub session_id: String,
    /// Workspace path.
    pub workspace: String,
    /// Current model identifier (e.g., "openai/kimi-k2.5").
    pub model: String,
    /// Number of messages in history.
    pub history_len: usize,
    /// Total tokens used in this session.
    pub total_tokens: u64,
    /// Estimated cost in USD.
    pub total_cost: f64,
    /// Registered tool names (builtin + MCP).
    pub tool_names: Vec<String>,
    /// Connected MCP servers and their tool counts: `(server_name, tool_count)`.
    pub mcp_servers: Vec<(String, usize)>,
}

/// Result of a slash command execution.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Text output to display to the user.
    pub text: String,
    /// Whether the command modified session state (e.g., /clear, /compact).
    pub state_changed: bool,
    /// Optional action for the session to perform after the command.
    pub action: Option<CommandAction>,
}

/// Post-command actions that the session should perform.
#[derive(Debug, Clone)]
pub enum CommandAction {
    /// Trigger context compaction.
    Compact,
    /// Clear conversation history.
    ClearHistory,
    /// Switch to a different model.
    SwitchModel(String),
}

impl CommandOutput {
    /// Create a simple text output.
    pub fn text(msg: impl Into<String>) -> Self {
        Self {
            text: msg.into(),
            state_changed: false,
            action: None,
        }
    }

    /// Create an output with a post-command action.
    pub fn with_action(msg: impl Into<String>, action: CommandAction) -> Self {
        Self {
            text: msg.into(),
            state_changed: true,
            action: Some(action),
        }
    }
}

/// Trait for implementing slash commands.
///
/// Implement this trait to add custom commands to the session.
pub trait SlashCommand: Send + Sync {
    /// Command name (without the leading `/`).
    fn name(&self) -> &str;

    /// Short description shown in `/help`.
    fn description(&self) -> &str;

    /// Optional usage hint (e.g., `/model <provider/model>`).
    fn usage(&self) -> Option<&str> {
        None
    }

    /// Execute the command with the given arguments.
    fn execute(&self, args: &str, ctx: &CommandContext) -> CommandOutput;
}

/// Registry of slash commands.
pub struct CommandRegistry {
    commands: HashMap<String, Arc<dyn SlashCommand>>,
}

impl CommandRegistry {
    /// Create a new registry with built-in commands.
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register(Arc::new(HelpCommand));
        registry.register(Arc::new(CompactCommand));
        registry.register(Arc::new(CostCommand));
        registry.register(Arc::new(ModelCommand));
        registry.register(Arc::new(ClearCommand));
        registry.register(Arc::new(HistoryCommand));
        registry.register(Arc::new(ToolsCommand));
        registry.register(Arc::new(McpCommand));
        registry
    }

    /// Register a custom command.
    pub fn register(&mut self, cmd: Arc<dyn SlashCommand>) {
        self.commands.insert(cmd.name().to_string(), cmd);
    }

    /// Unregister a command by name.
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn SlashCommand>> {
        self.commands.remove(name)
    }

    /// Check if input is a slash command.
    pub fn is_command(input: &str) -> bool {
        input.trim_start().starts_with('/')
    }

    /// Parse and execute a slash command. Returns `None` if not a command.
    pub fn dispatch(&self, input: &str, ctx: &CommandContext) -> Option<CommandOutput> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let without_slash = &trimmed[1..];
        let (name, args) = match without_slash.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (without_slash, ""),
        };

        match self.commands.get(name) {
            Some(cmd) => Some(cmd.execute(args, ctx)),
            None => Some(CommandOutput::text(format!(
                "Unknown command: /{name}\nType /help for available commands."
            ))),
        }
    }

    /// Get all registered command names and descriptions.
    pub fn list(&self) -> Vec<(&str, &str)> {
        let mut cmds: Vec<_> = self
            .commands
            .values()
            .map(|c| (c.name(), c.description()))
            .collect();
        cmds.sort_by_key(|(name, _)| *name);
        cmds
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Built-in Commands ──────────────────────────────────────────────

struct HelpCommand;

impl SlashCommand for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }
    fn description(&self) -> &str {
        "List available commands"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandOutput {
        // Help text is generated dynamically by the session using registry.list()
        // This is a placeholder — the actual help is built in AgentSession::execute_command()
        CommandOutput::text("Use /help to see available commands.")
    }
}

struct CompactCommand;

impl SlashCommand for CompactCommand {
    fn name(&self) -> &str {
        "compact"
    }
    fn description(&self) -> &str {
        "Manually trigger context compaction"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        CommandOutput::with_action(
            format!(
                "Compacting context... ({} messages, {} tokens)",
                ctx.history_len, ctx.total_tokens
            ),
            CommandAction::Compact,
        )
    }
}

struct CostCommand;

impl SlashCommand for CostCommand {
    fn name(&self) -> &str {
        "cost"
    }
    fn description(&self) -> &str {
        "Show token usage and estimated cost"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        CommandOutput::text(format!(
            "Session: {}\n\
             Model:   {}\n\
             Tokens:  {}\n\
             Cost:    ${:.4}",
            &ctx.session_id[..ctx.session_id.len().min(8)],
            ctx.model,
            ctx.total_tokens,
            ctx.total_cost,
        ))
    }
}

struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        "Show or switch the current model"
    }
    fn usage(&self) -> Option<&str> {
        Some("/model [provider/model]")
    }
    fn execute(&self, args: &str, ctx: &CommandContext) -> CommandOutput {
        if args.is_empty() {
            CommandOutput::text(format!("Current model: {}", ctx.model))
        } else if args.contains('/') {
            CommandOutput::with_action(
                format!("Switching model to: {args}"),
                CommandAction::SwitchModel(args.to_string()),
            )
        } else {
            CommandOutput::text(
                "Usage: /model provider/model (e.g., /model anthropic/claude-sonnet-4-20250514)",
            )
        }
    }
}

struct ClearCommand;

impl SlashCommand for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }
    fn description(&self) -> &str {
        "Clear conversation history"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        CommandOutput::with_action(
            format!("Cleared {} messages.", ctx.history_len),
            CommandAction::ClearHistory,
        )
    }
}

struct HistoryCommand;

impl SlashCommand for HistoryCommand {
    fn name(&self) -> &str {
        "history"
    }
    fn description(&self) -> &str {
        "Show conversation stats"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        CommandOutput::text(format!(
            "Messages: {}\n\
             Tokens:   {}\n\
             Session:  {}",
            ctx.history_len,
            ctx.total_tokens,
            &ctx.session_id[..ctx.session_id.len().min(8)],
        ))
    }
}

struct ToolsCommand;

impl SlashCommand for ToolsCommand {
    fn name(&self) -> &str {
        "tools"
    }
    fn description(&self) -> &str {
        "List registered tools"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        if ctx.tool_names.is_empty() {
            return CommandOutput::text("No tools registered.");
        }
        let builtin: Vec<&str> = ctx
            .tool_names
            .iter()
            .filter(|t| !t.starts_with("mcp__"))
            .map(|s| s.as_str())
            .collect();
        let mcp: Vec<&str> = ctx
            .tool_names
            .iter()
            .filter(|t| t.starts_with("mcp__"))
            .map(|s| s.as_str())
            .collect();

        let mut out = format!("Tools: {} total\n", ctx.tool_names.len());
        if !builtin.is_empty() {
            out.push_str(&format!("\nBuiltin ({}):\n", builtin.len()));
            for t in &builtin {
                out.push_str(&format!("  • {t}\n"));
            }
        }
        if !mcp.is_empty() {
            out.push_str(&format!("\nMCP ({}):\n", mcp.len()));
            for t in &mcp {
                out.push_str(&format!("  • {t}\n"));
            }
        }
        CommandOutput::text(out.trim_end())
    }
}

struct McpCommand;

impl SlashCommand for McpCommand {
    fn name(&self) -> &str {
        "mcp"
    }
    fn description(&self) -> &str {
        "List connected MCP servers and their tools"
    }
    fn execute(&self, _args: &str, ctx: &CommandContext) -> CommandOutput {
        if ctx.mcp_servers.is_empty() {
            return CommandOutput::text("No MCP servers connected.");
        }
        let total_tools: usize = ctx.mcp_servers.iter().map(|(_, c)| c).sum();
        let mut out = format!(
            "MCP: {} server(s), {} tool(s)\n",
            ctx.mcp_servers.len(),
            total_tools
        );
        for (server, count) in &ctx.mcp_servers {
            out.push_str(&format!("\n  {server} ({count} tools)"));
            // List tools belonging to this server
            let prefix = format!("mcp__{server}__");
            let server_tools: Vec<&str> = ctx
                .tool_names
                .iter()
                .filter(|t| t.starts_with(&prefix))
                .map(|s| s.strip_prefix(&prefix).unwrap_or(s))
                .collect();
            for t in server_tools {
                out.push_str(&format!("\n    • {t}"));
            }
        }
        CommandOutput::text(out)
    }
}

// ─── Scheduler-backed Commands ───────────────────────────────────────────────
//
// These commands require a shared `Arc<CronScheduler>` and are registered by
// `AgentSession::build_session()` after the scheduler is created, NOT by
// `CommandRegistry::new()`.

/// `/loop` — Schedule a recurring prompt.
///
/// Syntax (all equivalent):
/// - `/loop 5m check the deployment`
/// - `/loop check the deployment every 5m`
/// - `/loop check the deployment`  (defaults to every 10 minutes)
pub struct LoopCommand {
    pub scheduler: Arc<CronScheduler>,
}

impl SlashCommand for LoopCommand {
    fn name(&self) -> &str {
        "loop"
    }
    fn description(&self) -> &str {
        "Schedule a recurring prompt at a given interval"
    }
    fn usage(&self) -> Option<&str> {
        Some("/loop [interval] <prompt> [every <interval>]")
    }
    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandOutput {
        let args = args.trim();
        if args.is_empty() {
            return CommandOutput::text(concat!(
                "Usage: /loop [interval] <prompt> [every <interval>]\n\n",
                "Examples:\n",
                "  /loop 5m check the deployment status\n",
                "  /loop monitor memory usage every 2h\n",
                "  /loop check the build   (defaults to every 10m)\n\n",
                "Supported units: s (seconds), m (minutes), h (hours), d (days)",
            ));
        }
        let (interval, prompt) = parse_loop_args(args);
        match self.scheduler.create_task(prompt.clone(), interval, true) {
            Ok(id) => CommandOutput::text(format!(
                "Scheduled [{id}]: \"{prompt}\" — fires every {}",
                format_duration(interval.as_secs())
            )),
            Err(e) => CommandOutput::text(format!("Error: {e}")),
        }
    }
}

/// `/cron-list` — List all active scheduled tasks.
pub struct CronListCommand {
    pub scheduler: Arc<CronScheduler>,
}

impl SlashCommand for CronListCommand {
    fn name(&self) -> &str {
        "cron-list"
    }
    fn description(&self) -> &str {
        "List all scheduled recurring prompts"
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandOutput {
        let tasks = self.scheduler.list_tasks();
        if tasks.is_empty() {
            return CommandOutput::text(
                "No scheduled tasks. Use /loop to schedule a recurring prompt.",
            );
        }
        let mut out = format!("Scheduled tasks ({}):\n", tasks.len());
        for t in &tasks {
            let next = format_duration(t.next_fire_in_secs);
            let cadence = if t.recurring {
                format!("every {}", format_duration(t.interval_secs))
            } else {
                "once".to_string()
            };
            let preview = if t.prompt.len() > 60 {
                format!("{}…", &t.prompt[..60])
            } else {
                t.prompt.clone()
            };
            out.push_str(&format!(
                "  [{id}] {cadence} — fires in {next} (×{fires}) — \"{preview}\"\n",
                id = t.id,
                fires = t.fire_count,
            ));
        }
        CommandOutput::text(out.trim_end().to_string())
    }
}

/// `/cron-cancel <id>` — Cancel a scheduled task.
pub struct CronCancelCommand {
    pub scheduler: Arc<CronScheduler>,
}

impl SlashCommand for CronCancelCommand {
    fn name(&self) -> &str {
        "cron-cancel"
    }
    fn description(&self) -> &str {
        "Cancel a scheduled task by ID"
    }
    fn usage(&self) -> Option<&str> {
        Some("/cron-cancel <task-id>")
    }
    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandOutput {
        let id = args.trim();
        if id.is_empty() {
            return CommandOutput::text(
                "Usage: /cron-cancel <task-id>\nUse /cron-list to see active task IDs.",
            );
        }
        if self.scheduler.cancel_task(id) {
            CommandOutput::text(format!("Cancelled task [{id}]"))
        } else {
            CommandOutput::text(format!(
                "No task found with ID [{id}]. Use /cron-list to see active tasks."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> CommandContext {
        CommandContext {
            session_id: "test-session-123".into(),
            workspace: "/tmp/test".into(),
            model: "openai/kimi-k2.5".into(),
            history_len: 10,
            total_tokens: 5000,
            total_cost: 0.0123,
            tool_names: vec![
                "read".into(),
                "write".into(),
                "bash".into(),
                "mcp__github__create_issue".into(),
                "mcp__github__list_repos".into(),
            ],
            mcp_servers: vec![("github".into(), 2)],
        }
    }

    #[test]
    fn test_is_command() {
        assert!(CommandRegistry::is_command("/help"));
        assert!(CommandRegistry::is_command("  /model foo"));
        assert!(!CommandRegistry::is_command("hello"));
        assert!(!CommandRegistry::is_command("not /a command"));
    }

    #[test]
    fn test_dispatch_help() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/help", &ctx).unwrap();
        assert!(!out.text.is_empty());
    }

    #[test]
    fn test_dispatch_cost() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/cost", &ctx).unwrap();
        assert!(out.text.contains("5000"));
        assert!(out.text.contains("0.0123"));
    }

    #[test]
    fn test_dispatch_model_show() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/model", &ctx).unwrap();
        assert!(out.text.contains("openai/kimi-k2.5"));
        assert!(out.action.is_none());
    }

    #[test]
    fn test_dispatch_model_switch() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg
            .dispatch("/model anthropic/claude-sonnet-4-20250514", &ctx)
            .unwrap();
        assert!(matches!(out.action, Some(CommandAction::SwitchModel(_))));
    }

    #[test]
    fn test_dispatch_clear() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/clear", &ctx).unwrap();
        assert!(matches!(out.action, Some(CommandAction::ClearHistory)));
        assert!(out.text.contains("10"));
    }

    #[test]
    fn test_dispatch_compact() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/compact", &ctx).unwrap();
        assert!(matches!(out.action, Some(CommandAction::Compact)));
    }

    #[test]
    fn test_dispatch_unknown() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/foobar", &ctx).unwrap();
        assert!(out.text.contains("Unknown command"));
    }

    #[test]
    fn test_not_a_command() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        assert!(reg.dispatch("hello world", &ctx).is_none());
    }

    #[test]
    fn test_custom_command() {
        struct PingCommand;
        impl SlashCommand for PingCommand {
            fn name(&self) -> &str {
                "ping"
            }
            fn description(&self) -> &str {
                "Pong!"
            }
            fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandOutput {
                CommandOutput::text("pong")
            }
        }

        let mut reg = CommandRegistry::new();
        let before = reg.len();
        reg.register(Arc::new(PingCommand));
        assert_eq!(reg.len(), before + 1);

        let ctx = test_ctx();
        let out = reg.dispatch("/ping", &ctx).unwrap();
        assert_eq!(out.text, "pong");
    }

    #[test]
    fn test_list_commands() {
        let reg = CommandRegistry::new();
        let list = reg.list();
        assert!(list.len() >= 8);
        assert!(list.iter().any(|(name, _)| *name == "help"));
        assert!(list.iter().any(|(name, _)| *name == "compact"));
        assert!(list.iter().any(|(name, _)| *name == "cost"));
        assert!(list.iter().any(|(name, _)| *name == "mcp"));
    }

    #[test]
    fn test_dispatch_tools() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/tools", &ctx).unwrap();
        assert!(out.text.contains("5 total"));
        assert!(out.text.contains("read"));
        assert!(out.text.contains("mcp__github__create_issue"));
    }

    #[test]
    fn test_dispatch_mcp() {
        let reg = CommandRegistry::new();
        let ctx = test_ctx();
        let out = reg.dispatch("/mcp", &ctx).unwrap();
        assert!(out.text.contains("1 server(s)"));
        assert!(out.text.contains("github"));
        assert!(out.text.contains("create_issue"));
        assert!(out.text.contains("list_repos"));
    }

    #[test]
    fn test_dispatch_mcp_empty() {
        let reg = CommandRegistry::new();
        let mut ctx = test_ctx();
        ctx.mcp_servers = vec![];
        let out = reg.dispatch("/mcp", &ctx).unwrap();
        assert!(out.text.contains("No MCP servers connected"));
    }
}
