//! Host-facing run observability and control.
//!
//! Run lifecycle code owns mutation during execution. This module provides the
//! public control and query view over those run records.

use super::{run_lifecycle::RunControlState, AgentSession};

pub(super) struct RunControl<'a> {
    session: &'a AgentSession,
}

impl<'a> RunControl<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) async fn cancel_current(&self) -> bool {
        RunControlState::from_session(self.session).cancel().await
    }

    pub(super) async fn steer(
        &self,
        request: crate::run_control::SteerRequest,
    ) -> crate::error::Result<crate::run_control::RunControlReceipt> {
        let state = RunControlState::from_session(self.session);
        let Some(control) = state.current_run_control().await else {
            return Err(crate::error::CodeError::RunControl(
                crate::run_control::RunControlError::NoActiveRun,
            ));
        };
        let run_id = control.snapshot_run_id();
        let request = request.into_protocol(self.session.session_id(), &run_id);
        control
            .submit_with_hooks(request, self.session.config.host_env.now_ms())
            .await
            .map_err(crate::error::CodeError::from)
    }

    pub(super) async fn interrupt(
        &self,
        request: crate::run_control::InterruptRequest,
    ) -> crate::error::Result<crate::run_control::RunControlReceipt> {
        let state = RunControlState::from_session(self.session);
        let Some(control) = state.current_run_control().await else {
            return Err(crate::error::CodeError::RunControl(
                crate::run_control::RunControlError::NoActiveRun,
            ));
        };
        let run_id = control.snapshot_run_id();
        let request = request.into_protocol(self.session.session_id(), &run_id);
        control
            .submit_with_hooks(request, self.session.config.host_env.now_ms())
            .await
            .map_err(crate::error::CodeError::from)
    }

    pub(super) async fn run_control_snapshot(
        &self,
    ) -> Option<crate::run_control::RunControlSnapshot> {
        let control = RunControlState::from_session(self.session)
            .current_run_control()
            .await?;
        Some(control.snapshot().await)
    }
}

impl<'a> RunControl<'a> {
    pub(super) async fn cancel_run(&self, run_id: &str) -> bool {
        RunControlState::from_session(self.session)
            .cancel_run(run_id)
            .await
    }

    pub(super) async fn current_run(&self) -> Option<crate::run::RunHandle> {
        RunControlState::from_session(self.session)
            .current_run()
            .await
    }

    pub(super) async fn runs(&self) -> Vec<crate::run::RunSnapshot> {
        self.session.run_store.list().await
    }

    pub(super) async fn run_snapshot(&self, run_id: &str) -> Option<crate::run::RunSnapshot> {
        self.session.run_store.snapshot(run_id).await
    }

    pub(super) async fn run_events(&self, run_id: &str) -> Vec<crate::run::RunEventRecord> {
        self.session.run_store.events(run_id).await
    }

    pub(super) async fn run_event_page(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<crate::run::RunEventPage> {
        self.session
            .run_store
            .event_page(run_id, after_sequence, limit)
            .await
    }

    pub(super) async fn run_event_observation(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<crate::run::RunEventObservation> {
        self.session
            .run_store
            .event_observation(run_id, after_sequence, limit)
            .await
    }
}
