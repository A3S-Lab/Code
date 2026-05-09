//! Host-facing lane queue control.
//!
//! The queue is optional per session. This module keeps all none-vs-present
//! semantics in one place so the public facade can stay a stable thin surface.

use super::AgentSession;
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionLane, SessionQueueStats,
};
use a3s_lane::{DeadLetter, MetricsSnapshot};

pub(super) struct QueueControl<'a> {
    session: &'a AgentSession,
}

impl<'a> QueueControl<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) fn has_queue(&self) -> bool {
        self.session.command_queue.is_some()
    }

    pub(super) async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        if let Some(queue) = &self.session.command_queue {
            queue.set_lane_handler(lane, config).await;
        }
    }

    pub(super) async fn complete_external_task(
        &self,
        task_id: &str,
        result: ExternalTaskResult,
    ) -> bool {
        match &self.session.command_queue {
            Some(queue) => queue.complete_external_task(task_id, result).await,
            None => false,
        }
    }

    pub(super) async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        match &self.session.command_queue {
            Some(queue) => queue.pending_external_tasks().await,
            None => Vec::new(),
        }
    }

    pub(super) async fn stats(&self) -> SessionQueueStats {
        match &self.session.command_queue {
            Some(queue) => queue.stats().await,
            None => SessionQueueStats::default(),
        }
    }

    pub(super) async fn metrics(&self) -> Option<MetricsSnapshot> {
        match &self.session.command_queue {
            Some(queue) => queue.metrics_snapshot().await,
            None => None,
        }
    }

    pub(super) async fn dead_letters(&self) -> Vec<DeadLetter> {
        match &self.session.command_queue {
            Some(queue) => queue.dead_letters().await,
            None => Vec::new(),
        }
    }
}
