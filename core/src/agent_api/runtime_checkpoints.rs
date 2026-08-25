//! Exact checkpoint coordination at live Run boundaries.
//!
//! The agent loop owns logical execution state while the runtime event sink
//! owns the materialized Session view. This acknowledgement channel joins the
//! two without exposing an internal marker through the public `AgentEvent`
//! protocol: the event sink drains preceding events, captures the semantic
//! snapshot, publishes both components, and only then releases the loop.

use super::{session_persistence::SessionPersistenceContext, AgentSession};
use crate::loop_checkpoint::{LoopCheckpoint, LoopCheckpointSink};
use crate::session_checkpoint::{SessionCheckpointExportSink, SessionCheckpointExportV1};
use crate::store::SessionStore;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const CHECKPOINT_BOUNDARY_CHANNEL_CAPACITY: usize = 4;

pub(super) struct RuntimeCheckpointBoundary {
    checkpoint: LoopCheckpoint,
    acknowledgement: oneshot::Sender<()>,
}

pub(super) struct RuntimeCheckpointReceiver {
    rx: mpsc::Receiver<RuntimeCheckpointBoundary>,
    session_store: Option<Arc<dyn SessionStore>>,
    export_sink: Option<Arc<dyn SessionCheckpointExportSink>>,
    persistence: SessionPersistenceContext,
}

struct RuntimeCheckpointSender {
    tx: mpsc::Sender<RuntimeCheckpointBoundary>,
    session_store: Option<Arc<dyn SessionStore>>,
}

pub(super) fn runtime_checkpoint_channel(
    session: &AgentSession,
) -> (
    Option<Arc<dyn LoopCheckpointSink>>,
    RuntimeCheckpointReceiver,
) {
    let (tx, rx) = mpsc::channel(CHECKPOINT_BOUNDARY_CHANNEL_CAPACITY);
    let session_store = session.session_store.clone();
    let export_sink = session.session_checkpoint_export_sink.clone();
    let enabled = session_store.is_some() || export_sink.is_some();
    let sink = enabled.then(|| {
        Arc::new(RuntimeCheckpointSender {
            tx,
            session_store: session_store.clone(),
        }) as Arc<dyn LoopCheckpointSink>
    });
    (
        sink,
        RuntimeCheckpointReceiver {
            rx,
            session_store,
            export_sink,
            persistence: SessionPersistenceContext::from_session(session),
        },
    )
}

impl RuntimeCheckpointReceiver {
    pub(super) async fn recv(&mut self) -> Option<RuntimeCheckpointBoundary> {
        self.rx.recv().await
    }

    /// Persist a boundary after the owning event sink has drained all events
    /// that causally precede it. Every failure is isolated from the live Run.
    pub(super) async fn commit(&self, boundary: RuntimeCheckpointBoundary) {
        let RuntimeCheckpointBoundary {
            checkpoint,
            acknowledgement,
        } = boundary;

        let export = if self.export_sink.is_some() {
            match self
                .persistence
                .capture_checkpoint_snapshot(&checkpoint)
                .await
            {
                Ok(snapshot) => {
                    match SessionCheckpointExportV1::new(snapshot, Some(checkpoint.clone())) {
                        Ok(export) => Some(export),
                        Err(error) => {
                            tracing::warn!(
                                run_id = %checkpoint.run_id,
                                session_id = %checkpoint.session_id,
                                error = %error,
                                "Live Session checkpoint encoding failed; Run continues"
                            );
                            None
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %checkpoint.run_id,
                        session_id = %checkpoint.session_id,
                        error = %error,
                        "Live Session checkpoint snapshot failed; Run continues"
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Some(store) = &self.session_store {
            if let Err(error) = store
                .save_loop_checkpoint(&checkpoint.run_id, &checkpoint)
                .await
            {
                tracing::warn!(
                    run_id = %checkpoint.run_id,
                    session_id = %checkpoint.session_id,
                    error = %error,
                    "Loop checkpoint save failed; Run continues"
                );
            }
        }

        if let (Some(sink), Some(export)) = (&self.export_sink, export) {
            if let Err(error) = sink.export_checkpoint(export).await {
                tracing::warn!(
                    run_id = %checkpoint.run_id,
                    session_id = %checkpoint.session_id,
                    error = %error,
                    "Live Session checkpoint export failed; Run continues"
                );
            }
        }

        let _ = acknowledgement.send(());
    }
}

#[async_trait::async_trait]
impl LoopCheckpointSink for RuntimeCheckpointSender {
    async fn save_checkpoint(&self, checkpoint: &LoopCheckpoint) {
        let (acknowledgement, received) = oneshot::channel();
        let boundary = RuntimeCheckpointBoundary {
            checkpoint: checkpoint.clone(),
            acknowledgement,
        };
        if self.tx.send(boundary).await.is_ok() {
            // A dropped acknowledgement means the owning event sink already
            // stopped. The Run must keep its established best-effort behavior.
            let _ = received.await;
        }
    }

    async fn load_latest(&self, run_id: &str) -> Option<LoopCheckpoint> {
        let store = self.session_store.as_ref()?;
        match store.load_loop_checkpoint(run_id).await {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                tracing::warn!(
                    run_id,
                    error = %error,
                    "Loop checkpoint load failed"
                );
                None
            }
        }
    }
}
