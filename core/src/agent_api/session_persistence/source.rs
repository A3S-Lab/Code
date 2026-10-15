//! Source checks shared by save, resume and the original Run capture.

use super::*;
use a3s_effect::LogStore;

fn read_source(workspace: &Path, session_id: &str) -> Result<Vec<a3s_effect::Fact>> {
    let directory = crate::session_folder::effect_log_dir(workspace, session_id);
    if !directory.try_exists().map_err(source_error)? {
        return Ok(Vec::new());
    }
    a3s_effect::FileLog::open(directory)
        .and_then(|log| log.read(&crate::fact_control::thread_for_session(session_id)))
        .map_err(source_error)
}

fn source_error(error: impl std::fmt::Display) -> CodeError {
    CodeError::Session(format!("durable session source: {error}"))
}

pub(in crate::agent_api) fn restore_source(
    data: &SessionData,
    workspace: &Path,
    requested: Option<&crate::WorkPolicyV1>,
) -> Result<Option<crate::WorkPolicyV1>> {
    if let Some(source) = &data.durable_source {
        let facts = read_source(workspace, &data.id)?;
        source.verify(&data.id, &facts).map_err(source_error)?;
        return crate::work_policy::prepare_policy(
            &facts,
            &crate::fact_control::thread_for_session(&data.id),
            requested,
        )
        .map(|prepared| prepared.policy)
        .map_err(source_error);
    }
    Ok(None)
}

impl AgentSession {
    pub(in crate::agent_api) fn saved_durable_source(
        &self,
    ) -> Option<crate::store::SessionFactSourceV1> {
        read_or_recover(&self.persistence_state)
            .baseline
            .as_ref()
            .and_then(|data| data.durable_source.clone())
            .or_else(|| self.config.durable_source.clone())
    }

    pub(in crate::agent_api) fn verify_saved_source(
        &self,
        facts: &[a3s_effect::Fact],
    ) -> Result<()> {
        if let Some(source) = self.saved_durable_source() {
            source
                .verify(&self.session_id, facts)
                .map_err(source_error)?;
        }
        Ok(())
    }
}

impl SessionPersistenceContext {
    pub(in crate::agent_api) fn validate_checkpoint_source(
        &self,
        checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
    ) -> Result<()> {
        checkpoint
            .validate_source(self.capture_source()?.as_ref())
            .map_err(source_error)
    }

    pub(in crate::agent_api) fn capture_source(
        &self,
    ) -> Result<Option<crate::store::SessionFactSourceV1>> {
        let cached = self.fact_log.lock().map_err(source_error)?.clone();
        let facts = match cached {
            Some(log) => log
                .read(&crate::fact_control::thread_for_session(&self.session_id))
                .map_err(source_error)?,
            None => read_source(&self.workspace, &self.session_id)?,
        };
        self.source_from_facts(&facts)
    }

    pub(in crate::agent_api) fn source_from_facts(
        &self,
        facts: &[a3s_effect::Fact],
    ) -> Result<Option<crate::store::SessionFactSourceV1>> {
        let prior = read_or_recover(&self.persistence_state)
            .baseline
            .as_ref()
            .and_then(|data| data.durable_source.clone())
            .or_else(|| self.config.durable_source.clone());
        if let Some(source) = prior {
            source
                .verify(&self.session_id, facts)
                .map_err(source_error)?;
        }
        let thread = crate::fact_control::thread_for_session(&self.session_id);
        let policy =
            crate::work_policy::prepare_policy(facts, &thread, self.config.work_policy.as_ref())
                .map_err(source_error)?;
        if policy.policy.is_some()
            && !facts
                .iter()
                .any(|fact| fact.kind == crate::work_policy::POLICY_KIND)
        {
            return Err(source_error(
                "requested policy has no admitted original source",
            ));
        }
        let digest = policy
            .policy
            .as_ref()
            .map(crate::WorkPolicyV1::digest)
            .transpose()
            .map_err(source_error)?;
        crate::goal_control::definition_from_facts(
            facts,
            &self.session_id,
            self.tool_executor
                .registry()
                .context()
                .workspace_services
                .workspace_ref(),
            digest.as_deref(),
        )
        .map_err(source_error)?;
        crate::store::SessionFactSourceV1::from_facts(&self.session_id, facts).map_err(source_error)
    }
}
