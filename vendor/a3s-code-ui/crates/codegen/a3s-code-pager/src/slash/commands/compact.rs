//! `/compact` takes an optional focus argument.
//! `run` returns `CommandResult::QueueCommand`. Dispatch drains that as
//! `Effect::Compact`, which calls `x.ai/compact_conversation` on the ACP agent.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Compact the conversation history, optionally with a focus context.
pub struct CompactCommand;

impl SlashCommand for CompactCommand {
    slash_meta! {
        name: "compact",
        description: "Compact conversation history",
        usage: "/compact compaction instructions",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "compaction instructions",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        // Re-emit as queue command, preserving the full text.
        let text = if args.trim().is_empty() {
            "/compact".to_string()
        } else {
            format!("/compact {}", args)
        };
        CommandResult::QueueCommand(text)
    }
}
