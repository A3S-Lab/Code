//! Agent commands use one runtime for backend selection and leader resolution.
use crate::app::{AgentArgs, AgentCmd, LeaderMode, resolve_leader_mode};
use a3s_code_shell::agent::config::Config;
use anyhow::Result;
pub struct AgentRuntime {
    backend: Backend,
    leader_mode: LeaderMode<'static>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Shell,
}
impl AgentRuntime {
    pub fn from_args(args: &AgentArgs, config: &Config, raw_config: &toml::Value) -> AgentRuntime {
        Backend::configured().resolve(args, config, raw_config)
    }
    pub fn leader_mode(&self) -> LeaderMode<'static> {
        self.leader_mode
    }
    pub async fn run_stdio(&self, config: &Config) -> Result<()> {
        match self.backend {
            Backend::Shell => {
                a3s_code_shell::agent::app::run_stdio_agent(
                    config,
                    None,
                    config.memory_config.clone(),
                )
                .await
            }
        }
    }
}
impl Backend {
    fn configured() -> Backend {
        Backend::Shell
    }
    fn resolve(self, args: &AgentArgs, config: &Config, raw_config: &toml::Value) -> AgentRuntime {
        let backend = self.for_command(args.mode.as_ref());
        let leader_mode = match backend {
            Backend::Shell => resolve_leader_mode(
                args.leader,
                args.no_leader,
                raw_config,
                config.remote_settings.as_ref(),
                matches!(
                    &args.mode,
                    None | Some(AgentCmd::Stdio) | Some(AgentCmd::Headless(_))
                ),
                a3s_code_sandbox::requested_confinement_profile(),
            ),
        };
        AgentRuntime {
            backend,
            leader_mode,
        }
    }
    fn for_command(self, command: Option<&AgentCmd>) -> Backend {
        match command {
            Some(AgentCmd::Stdio) => self,
            None | Some(AgentCmd::Headless(_) | AgentCmd::Serve(_) | AgentCmd::Leader(_)) => {
                Backend::Shell
            }
        }
    }
}
#[cfg(test)]
#[path = "agent_runtime_tests.rs"]
mod tests;
