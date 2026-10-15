//! New-session history forks reuse the original source and target admission resources.

use super::*;
use crate::store::SessionSnapshotV1;
use a3s_effect::LogStore;

fn fork_error(error: impl std::fmt::Display) -> CodeError {
    CodeError::Session(format!("session fork: {error}"))
}

pub(in crate::agent_api) async fn fork_session_async(
    agent: &Agent,
    snapshot: SessionSnapshotV1,
    workspace: String,
    mut options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;
    snapshot
        .validate_for_session(&snapshot.session.id)
        .map_err(fork_error)?;
    let target_id = required_session_id(&options)?.to_owned();
    if target_id.trim().is_empty() || target_id == snapshot.session.id {
        return Err(fork_error(
            "an explicit different target session is required",
        ));
    }
    if snapshot
        .run_records
        .iter()
        .any(|record| !record.snapshot.status.is_terminal())
        || snapshot.subagent_tasks.iter().any(|task| {
            matches!(
                task.status,
                crate::subagent_task_tracker::SubagentStatus::Running
            )
        })
    {
        return Err(fork_error("source has unfinished execution"));
    }

    let source_workspace = std::path::Path::new(&snapshot.session.config.workspace);
    let source_directory =
        crate::session_folder::effect_log_dir(source_workspace, &snapshot.session.id);
    if snapshot.session.durable_source.is_some()
        && !source_directory.try_exists().map_err(fork_error)?
    {
        return Err(fork_error("durable source is missing"));
    }
    let source = Arc::new(a3s_effect::FileLog::open(&source_directory).map_err(fork_error)?);
    let source_thread = crate::fact_control::thread_for_session(&snapshot.session.id);
    let _source_owner = source.try_run_owner(&source_thread).map_err(fork_error)?;
    let _source_writer = source.try_writer_fence().map_err(fork_error)?;
    let facts = source.read(&source_thread).map_err(fork_error)?;
    if let Some(binding) = &snapshot.session.durable_source {
        binding
            .verify(&snapshot.session.id, &facts)
            .map_err(fork_error)?;
    } else if crate::store::SessionFactSourceV1::from_facts(&snapshot.session.id, &facts)
        .map_err(fork_error)?
        .is_some()
    {
        return Err(fork_error("snapshot omits the current durable source"));
    }
    let policy =
        crate::work_policy::prepare_policy(&facts, &source_thread, None).map_err(fork_error)?;
    if policy.policy.as_ref() != options.work_policy.as_ref() {
        return Err(fork_error(
            "source policy requires explicit identical acceptance",
        ));
    }
    let digest = policy
        .policy
        .as_ref()
        .map(crate::WorkPolicyV1::digest)
        .transpose()
        .map_err(fork_error)?;
    crate::goal_control::definition_from_facts(
        &facts,
        &snapshot.session.id,
        crate::workspace::WorkspaceServices::local(source_workspace).workspace_ref(),
        digest.as_deref(),
    )
    .map_err(fork_error)?;
    if crate::fact_control::has_unsettled_effects(&facts) {
        return Err(fork_error("source has unsettled effects"));
    }

    session_persistence::ensure_artifact_restore_capacity(&mut options, &snapshot);
    let options = session_builder::prepare_session_options(agent, options);
    let reservation = reserve_session(agent, &target_id)?;
    let canonical = super::super::safe_canonicalize(std::path::Path::new(&workspace));
    let resolved =
        session_config::ResolvedSessionConfig::resolve(agent, &canonical, options).await?;
    let session = session_builder::build_agent_session(agent, workspace, resolved).await?;
    let admitted = async {
        if crate::session_folder::effect_log_dir(session.workspace(), &target_id)
            == source_directory
        {
            // Flat legacy sessions share one directory writer, with distinct thread owners.
            let slot = session.run_admission.fact_log_slot();
            let mut cached = slot.lock().map_err(fork_error)?;
            if cached.is_some() {
                return Err(fork_error("unpublished target already has a log handle"));
            }
            *cached = Some(Arc::clone(&source));
        }
        let lease = session.run_admission.try_acquire(&target_id)?;
        let log = lease.nonexecuting_fact_log(&session.run_admission, &target_id)?;
        let writer = log.try_writer_fence().map_err(fork_error)?;
        let thread = crate::fact_control::thread_for_session(&target_id);
        let target_facts = log.read(&thread).map_err(fork_error)?;
        if !target_facts.is_empty() {
            return Err(fork_error("target already has facts"));
        }
        if let Some(store) = &session.session_store {
            if store.exists(&target_id).await.map_err(fork_error)? {
                return Err(fork_error("target already has a saved session"));
            }
        }
        bail_if_agent_closed(agent)?;
        let prepared =
            crate::work_policy::prepare_policy(&target_facts, &thread, policy.policy.as_ref())
                .map_err(fork_error)?;
        session.admit_nonexecuting_standing(&log, &target_facts, &thread)?;
        crate::work_policy::commit_policy(&log, &writer, &thread, &prepared).map_err(fork_error)?;
        crate::fact_control::seed_history(&log, &thread, &snapshot.session.messages)
            .map_err(fork_error)?;
        let persistence = session_persistence::SessionPersistenceContext::from_session(&session);
        let fresh = persistence.capture_snapshot().await?;
        let fork = snapshot
            .fork_with_runtime_state(fresh.session)
            .map_err(fork_error)?;
        session_persistence::restore_persisted_session_state(&session, fork).await?;
        persistence.save().await?;
        Ok(())
    }
    .await;
    if let Err(error) = admitted {
        session.close().await;
        return Err(error);
    }
    if let Err(error) = reservation.finalize(&session.close_handle) {
        session.close().await;
        return Err(error);
    }
    Ok(session)
}
