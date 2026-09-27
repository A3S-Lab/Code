//! `/goal` sets one durable objective for the ACP session.
//!
//! The pager sends the command text. `a3s-code-acp` stores the objective and
//! turns Core planning on or off. Status, pause, and clear do not start a
//! model turn.

use agent_client_protocol as acp;

use crate::slash::command::{ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Set, inspect, pause, resume, or clear the session goal.
pub struct GoalCommand;

impl SlashCommand for GoalCommand {
    slash_meta! {
        name: "goal",
        description: "Set, pause, resume, or clear a durable goal",
        usage: "/goal <objective> | status | pause | resume | clear",
        takes_args: true,
        args_required: false,
        session_scoped: true,
        arg_placeholder: "<objective> | status | pause | resume | clear",
    }

    fn suggest_args(
        &self,
        _ctx: &crate::slash::command::AppCtx,
        args_query: &str,
    ) -> Option<Vec<ArgItem>> {
        let query = args_query.trim().to_ascii_lowercase();
        let items: Vec<ArgItem> = ["status", "pause", "resume", "clear"]
            .into_iter()
            .filter(|name| query.is_empty() || name.starts_with(query.as_str()))
            .map(|name| ArgItem {
                display: name.to_string(),
                match_text: name.to_string(),
                insert_text: name.to_string(),
                description: match name {
                    "status" => "Show the current goal",
                    "pause" => "Stop planning on later turns",
                    "resume" => "Continue the current goal",
                    "clear" => "Drop the current goal",
                    _ => "",
                }
                .to_string(),
            })
            .collect();
        if items.is_empty() { None } else { Some(items) }
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let body = args.trim();
        let text = if body.is_empty() {
            "/goal".to_string()
        } else {
            format!("/goal {body}")
        };
        CommandResult::InjectSkill {
            display_text: text.clone(),
            prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
            display_as_skill: false,
            scheduled_task_preview: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    static DEFAULT_BUNDLE_STATE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn run(args: &str) -> CommandResult {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Minimal,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        };
        GoalCommand.run(&mut ctx, args)
    }

    #[test]
    fn empty_goal_is_sent_as_the_status_command() {
        match run("") {
            CommandResult::InjectSkill {
                display_text,
                prompt_blocks,
                display_as_skill,
                ..
            } => {
                assert_eq!(display_text, "/goal");
                assert!(!display_as_skill);
                assert!(matches!(
                    prompt_blocks.first(),
                    Some(acp::ContentBlock::Text(text)) if text.text == "/goal"
                ));
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }

    #[test]
    fn objective_keeps_its_text() {
        match run("  Ship the fix  ") {
            CommandResult::InjectSkill { display_text, .. } => {
                assert_eq!(display_text, "/goal Ship the fix");
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }
}
