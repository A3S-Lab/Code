//! A3S slash-command catalog for the Code TUI.
//!
//! Fuzzy matching is vendored from the a3s-build pager. The command list,
//! groups, and browse-hidden set are A3S product surface (ported from
//! `crates/cli` `ui/chrome.rs`).

mod matcher;
mod parse;

pub use matcher::FuzzyMatcher;
pub use parse::{parse_invocation, SlashInvocation};

/// Maximum number of visible rows in the dropdown (scroll beyond this).
pub const MAX_VISIBLE_SUGGESTIONS: usize = 8;

/// One slash command offered by the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    /// Token after `/` (no leading slash).
    pub name: &'static str,
    pub description: &'static str,
    pub group: SlashCommandGroup,
}

/// Stable information-architecture buckets shared by the slash menu and `/help`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlashCommandGroup {
    Workflow,
    Session,
    Context,
    Assets,
    System,
}

impl SlashCommandGroup {
    pub const ALL: [Self; 5] = [
        Self::Workflow,
        Self::Session,
        Self::Context,
        Self::Assets,
        Self::System,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow",
            Self::Session => "Session & control",
            Self::Context => "Context & memory",
            Self::Assets => "Assets & services",
            Self::System => "System & interface",
        }
    }
}

/// Full A3S coding slash catalog (names without leading `/`).
pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "status",
        description: "show session, workspace, model, permission mode, and token usage",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "model",
        description: "switch configured/account models",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "permissions",
        description: "change the next-turn permission mode or inspect exact grants",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "sandbox",
        description: "show host Bash sandbox readiness and fail-closed boundary status",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "display",
        description: "cycle status-meter density",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "statusline",
        description: "status-line decorator · /statusline clear",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "hooks",
        description: "inspect, trust, disable, enable, or reload lifecycle hooks",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "review",
        description: "review working tree, commit, or branch without changing files",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "reviewer",
        description: "toggle sticky claim-vs-record reply verifier",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "ask",
        description: "read-only explore (alias for plan)",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "plan",
        description: "read-only planning mode",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "ide",
        description: "built-in file browser + editor",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "tasks",
        description: "inspect delegated work · search, view output, or cancel safely",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "queue",
        description: "inspect pending follow-ups · send now, remove, or clear",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "history",
        description: "fuzzy-search prompts from the current session",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "help",
        description: "show grouped commands and shortcuts",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "init",
        description: "analyze the project and generate AGENTS.md",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "config",
        description: "edit config.acl in the built-in editor",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "terminal",
        description: "terminal capabilities + Shift+Enter / multiplexer repair snippets",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "checkup",
        description: "audit setup, then review proposed fixes before applying them",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "copy",
        description: "copy the latest response · add `transcript` for the semantic session",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "export",
        description: "write a new Markdown session file · optional workspace-relative path",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "use",
        description: "integrations hub · /use [status|repair|plugin|packages|reload]",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "theme",
        description: "cycle highlight theme",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "login",
        description: "sign in to the configured OS account",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "logout",
        description: "sign out from the configured OS account",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "plugin",
        description: "skills & plugins · prefer /use plugin",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "packages",
        description: "Use packages · prefer /use packages",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "reload",
        description: "re-scan skills/plugins · prefer /use reload",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "update",
        description: "upgrade a3s to the latest release",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "desktop",
        description: "open this workspace in the latest A3S Desktop",
        group: SlashCommandGroup::Assets,
    },
    SlashCommand {
        name: "memory",
        description: "memory graph · prefer /ctx memory",
        group: SlashCommandGroup::Context,
    },
    SlashCommand {
        name: "evolution",
        description: "learned prefs/skills · prefer /ctx evolution",
        group: SlashCommandGroup::Context,
    },
    SlashCommand {
        name: "research",
        description: "deep research hub · /research <query>",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "kb",
        description: "personal knowledge base · prefer /ctx kb",
        group: SlashCommandGroup::Context,
    },
    SlashCommand {
        name: "ctx",
        description: "context hub · /ctx <query> · memory|kb|sleep|evolution",
        group: SlashCommandGroup::Context,
    },
    SlashCommand {
        name: "effort",
        description: "adjust model effort (low … ultracode)",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "compact",
        description: "summarize + compact the conversation context",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "goal",
        description: "durable Ultracode goal",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "loop",
        description: "engineered loop dashboard",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "sleep",
        description: "consolidate today's work · prefer /ctx sleep",
        group: SlashCommandGroup::Context,
    },
    SlashCommand {
        name: "relay",
        description: "session handoff · resume/pin existing sessions",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "fork",
        description: "session branch · copy transcript · add `worktree` for isolated git",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "worktree",
        description: "isolation · managed worktree status/handoff/cleanup",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "isolate",
        description: "promote, discard, or record accept/revert/reject for isolation worktree",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "rewind",
        description: "undo the last completed turn when its files still match",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "clear",
        description: "reset the conversation",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "unstick",
        description: "clear sticky skill mode",
        group: SlashCommandGroup::Workflow,
    },
    SlashCommand {
        name: "auto",
        description: "alias for Shift+Tab → auto",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "yolo",
        description: "alias for Shift+Tab → yolo",
        group: SlashCommandGroup::System,
    },
    SlashCommand {
        name: "exit",
        description: "quit a3s code",
        group: SlashCommandGroup::Session,
    },
    SlashCommand {
        name: "quit",
        description: "quit a3s code (alias for /exit)",
        group: SlashCommandGroup::Session,
    },
];

/// Commands kept for typed dispatch and `/help`, omitted from empty `/` browse.
pub const SLASH_BROWSE_HIDDEN: &[&str] = &[
    "auto",
    "yolo",
    "theme",
    "display",
    "statusline",
    "terminal",
    "memory",
    "evolution",
    "kb",
    "sleep",
    "plugin",
    "packages",
    "reload",
    "ide",
    "goal",
    "loop",
    "quit",
];

/// Command token after `/`, without arguments.
pub fn parse_slash(line: &str) -> Option<&str> {
    parse_invocation(line.trim()).map(|invocation| invocation.token)
}

/// Look up a catalog entry by token name.
pub fn find_command(name: &str) -> Option<&'static SlashCommand> {
    SLASH_COMMANDS.iter().find(|command| command.name == name)
}

/// Menu rows ranked by the pager's fuzzy matcher (browse-hidden filtered when query empty).
pub fn matching_commands(line: &str) -> Vec<&'static SlashCommand> {
    let query = line
        .trim()
        .strip_prefix('/')
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("");
    let candidates: Vec<&SlashCommand> = if query.is_empty() {
        SLASH_COMMANDS
            .iter()
            .filter(|command| !SLASH_BROWSE_HIDDEN.contains(&command.name))
            .collect()
    } else {
        SLASH_COMMANDS.iter().collect()
    };
    let mut matcher = FuzzyMatcher::new();
    matcher
        .rank(&candidates, query, MAX_VISIBLE_SUGGESTIONS, |command| {
            command.name
        })
        .into_iter()
        .filter_map(|(index, _score)| candidates.get(index).copied())
        .collect()
}
