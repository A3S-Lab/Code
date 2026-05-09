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
}
