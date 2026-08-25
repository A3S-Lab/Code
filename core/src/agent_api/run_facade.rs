use super::*;

impl AgentSession {
    /// Cancel a specific run only if it is still the active run.
    ///
    /// This is useful for SDK callers that hold a previously observed run ID:
    /// stale run IDs will not cancel a newer operation.
    pub async fn cancel_run(&self, run_id: &str) -> bool {
        RunControl::from_session(self).cancel_run(run_id).await
    }

    /// Return snapshots for runs recorded by this session.
    pub async fn runs(&self) -> Vec<crate::run::RunSnapshot> {
        RunControl::from_session(self).runs().await
    }

    /// Return a snapshot for a recorded run.
    pub async fn run_snapshot(&self, run_id: &str) -> Option<crate::run::RunSnapshot> {
        RunControl::from_session(self).run_snapshot(run_id).await
    }

    /// Return recorded runtime events for a run.
    pub async fn run_events(&self, run_id: &str) -> Vec<crate::run::RunEventRecord> {
        RunControl::from_session(self).run_events(run_id).await
    }

    /// Return a cursor-based page from the run's retained event window.
    ///
    /// `after_sequence` is exclusive. The result is `None` for an unknown run;
    /// a known run with no retained events returns an empty page. Inspect
    /// `retention_gap` before treating the page as complete history.
    pub async fn run_event_page(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<crate::run::RunEventPage> {
        RunControl::from_session(self)
            .run_event_page(run_id, after_sequence, limit)
            .await
    }

    /// Return one internally consistent snapshot and event page generation.
    pub(crate) async fn run_event_observation(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<crate::run::RunEventObservation> {
        RunControl::from_session(self)
            .run_event_observation(run_id, after_sequence, limit)
            .await
    }

    /// Return a handle for the currently running operation, if any.
    pub async fn current_run(&self) -> Option<crate::run::RunHandle> {
        RunControl::from_session(self).current_run().await
    }

    /// Return active tool calls observed for the currently running operation.
    pub async fn active_tools(&self) -> Vec<crate::run::ActiveToolSnapshot> {
        SessionView::from_session(self).active_tools().await
    }

    /// Look up a delegated subagent task by id. Returns `None` if no such task
    /// has been observed in this session.
    pub async fn subagent_task(
        &self,
        task_id: &str,
    ) -> Option<crate::subagent_task_tracker::SubagentTaskSnapshot> {
        self.subagent_tasks.get(task_id).await
    }

    /// Return snapshots of every delegated subagent task observed in this
    /// session (including completed and failed ones), oldest first.
    pub async fn subagent_tasks(&self) -> Vec<crate::subagent_task_tracker::SubagentTaskSnapshot> {
        self.subagent_tasks.list_for_parent(&self.session_id).await
    }

    /// Return snapshots of subagent tasks still in `Running` state.
    pub async fn pending_subagent_tasks(
        &self,
    ) -> Vec<crate::subagent_task_tracker::SubagentTaskSnapshot> {
        use crate::subagent_task_tracker::SubagentStatus;
        self.subagent_tasks
            .list_for_parent(&self.session_id)
            .await
            .into_iter()
            .filter(|task| task.status == SubagentStatus::Running)
            .collect()
    }

    /// Cancel an in-flight delegated subagent task by id. Returns `true`
    /// when a cancellation token was found and fired, `false` when the
    /// task id is unknown or the task has already finished. The eventual
    /// `SubagentEnd` from the cancelled child loop won't downgrade the
    /// terminal status — it stays `Cancelled`.
    pub async fn cancel_subagent_task(&self, task_id: &str) -> bool {
        self.subagent_tasks.cancel(task_id).await
    }

    /// Return a shared handle to the session's subagent task tracker.
    ///
    /// Advanced: embedders implementing a custom subagent execution path
    /// (i.e. spawning child loops outside the built-in `task` tool) can use
    /// this to register cancellation tokens and feed `AgentEvent`s into the
    /// tracker so the standard
    /// [`subagent_task`](Self::subagent_task) / [`pending_subagent_tasks`](Self::pending_subagent_tasks) /
    /// [`cancel_subagent_task`](Self::cancel_subagent_task) APIs and
    /// [`close`](Self::close) keep working uniformly across execution paths.
    pub fn subagent_tracker(
        &self,
    ) -> Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker> {
        Arc::clone(&self.subagent_tasks)
    }
}
