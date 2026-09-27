//! `/config`: open the provider and model modal.
//!
//! This is the TUI counterpart of the model page in product settings: pick a
//! provider, edit its connection and models, then save `config.acl`.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};
use crate::views::model_config::ModelConfigState;

pub struct ConfigCommand;

impl SlashCommand for ConfigCommand {
    slash_meta! {
        name: "config",
        description: "Add and configure providers and models",
        usage: "/config",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenModelConfig(Box::new(
            ModelConfigState::load_default(),
        )))
    }
}
