//! Slash commands the agent handles inside a prompt, announced to the client
//! with the ACP `available_commands_update` session notification.

use super::A3sCodeAgent;
use agent_client_protocol::{self as acp, Client as _};

/// Every slash command `prompt` handles. The `initialize` metadata and the
/// session notification both read this list.
pub(super) fn available_commands() -> Vec<acp::AvailableCommand> {
    vec![
        acp::AvailableCommand::new("goal", "Set, pause, resume, or clear a durable goal").input(
            acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new(
                "<objective> | status | pause | resume | clear",
            )),
        ),
    ]
}

impl A3sCodeAgent {
    /// Announce the commands once the session response has been written: the
    /// client only routes updates for a session id it has already received.
    pub(super) fn announce_commands(&self, session_id: &str) {
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        let notification = acp::SessionNotification::new(
            acp::SessionId::new(session_id.to_string()),
            acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                available_commands(),
            )),
        );
        tokio::task::spawn_local(async move {
            tokio::task::yield_now().await;
            let _ = client.session_notification(notification).await;
        });
    }
}
