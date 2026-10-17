//! The closed set of host extension methods the agent answers beyond ACP.
//!
//! `ext_method` parses the wire method into a [`HostMethod`]; a method
//! outside the set is `method_not_found`. Goal, review, wiki, compaction,
//! interjection, session catalog, and permission posture are host features,
//! not ACP protocol.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostMethod {
    GoalImport,
    GoalImportPreview,
    GoalCommand,
    Review,
    WikiSearch,
    CompactConversation,
    Interject,
    SessionInfo,
    SessionList,
    SessionDelete,
    PermissionPosture,
}

impl HostMethod {
    #[cfg(test)]
    pub(super) const ALL: [Self; 11] = [
        Self::GoalImport,
        Self::GoalImportPreview,
        Self::GoalCommand,
        Self::Review,
        Self::WikiSearch,
        Self::CompactConversation,
        Self::Interject,
        Self::SessionInfo,
        Self::SessionList,
        Self::SessionDelete,
        Self::PermissionPosture,
    ];

    /// The wire method, or `None` outside the closed set. Compaction and
    /// interjection keep their `x.ai/` prefix families.
    pub(super) fn parse(method: &str) -> Option<Self> {
        Some(match method {
            "a3s/goal/import" => Self::GoalImport,
            "a3s/goal/import/preview" => Self::GoalImportPreview,
            "a3s/goal/definition"
            | "a3s/goal/plan"
            | "a3s/goal/control"
            | "a3s/goal/wait"
            | "a3s/goal/state" => Self::GoalCommand,
            "a3s/review/target" | "a3s/review/admit" | "a3s/review/settle" => Self::Review,
            "a3s/wiki/search" => Self::WikiSearch,
            "x.ai/session/info" => Self::SessionInfo,
            "x.ai/session/list" => Self::SessionList,
            "x.ai/session/delete" => Self::SessionDelete,
            "x.ai/permission_posture" => Self::PermissionPosture,
            _ if method.starts_with("x.ai/compact_conversation") => Self::CompactConversation,
            _ if method.starts_with("x.ai/interject") => Self::Interject,
            _ => return None,
        })
    }

    /// The method name, or the family prefix for a family of methods.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::GoalImport => "a3s/goal/import",
            Self::GoalImportPreview => "a3s/goal/import/preview",
            Self::GoalCommand => "a3s/goal/",
            Self::Review => "a3s/review/",
            Self::WikiSearch => "a3s/wiki/search",
            Self::CompactConversation => "x.ai/compact_conversation",
            Self::Interject => "x.ai/interject",
            Self::SessionInfo => "x.ai/session/info",
            Self::SessionList => "x.ai/session/list",
            Self::SessionDelete => "x.ai/session/delete",
            Self::PermissionPosture => "x.ai/permission_posture",
        }
    }
}

impl A3sCodeAgent {
    pub(super) async fn answer_host_method(
        &self,
        method: HostMethod,
        args: &acp::ExtRequest,
    ) -> acp::Result<acp::ExtResponse> {
        tracing::debug!(method = method.as_str(), "host extension method");
        match method {
            HostMethod::GoalImport => self.import_goal(args).await,
            HostMethod::GoalImportPreview => self.preview_goal_import(args).await,
            HostMethod::GoalCommand => self.handle_goal_command(args).await,
            HostMethod::Review => self.handle_review(args).await,
            HostMethod::WikiSearch => answer_wiki_search(args).await,
            HostMethod::CompactConversation => self.handle_compact_extension(args).await,
            HostMethod::Interject => self.handle_interject(args).await,
            HostMethod::SessionInfo => self.handle_session_info(args).await,
            HostMethod::SessionList => self.handle_session_list(args).await,
            HostMethod::SessionDelete => self.handle_session_delete(args).await,
            HostMethod::PermissionPosture => self.handle_permission_posture(args).await,
        }
    }

    async fn import_goal(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let command: a3s_code_core::goal_control::GoalDefinitionCommandV1 =
            serde_json::from_str(args.params.get())
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        let session_id = command.session_id.clone();
        let (session, turn_queue) = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|live| (Arc::clone(&live.session), Arc::clone(&live.turn_queue)))
            .ok_or_else(|| acp::Error::resource_not_found(None))?;
        // A retiring legacy turn can still remove its source after Core returns.
        let _turn_permit = turn_queue
            .acquire_owned()
            .await
            .ok_or_else(|| acp::Error::invalid_params().data("goal import queue is full"))?;
        let imported = crate::goal::commit_import(&session, command)
            .await
            .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        self.goals.lock().await.remove(&session_id);
        let raw = serde_json::value::to_raw_value(&imported)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    async fn preview_goal_import(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request: crate::goal::GoalImportPreviewRequest =
            serde_json::from_str(args.params.get())
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        let session = self
            .sessions
            .lock()
            .await
            .get(&request.session_id)
            .map(|live| Arc::clone(&live.session))
            .ok_or_else(|| acp::Error::resource_not_found(None))?;
        if session.is_closed() {
            return Err(acp::Error::resource_not_found(None));
        }
        if session
            .goal_definition()
            .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?
            .is_some()
        {
            return Err(acp::Error::invalid_params().data("goal.conflict: session already bound"));
        }
        let preview = crate::goal::preview_import(session.workspace(), &request)
            .await
            .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        let raw = serde_json::value::to_raw_value(&preview)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }
}

#[cfg(test)]
#[path = "host_methods_tests.rs"]
mod tests;
