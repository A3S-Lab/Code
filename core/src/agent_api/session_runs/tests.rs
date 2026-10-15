#![cfg(test)]

use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
use a3s_effect::{FileLog, LogStore, NewFact};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct NoDispatch(AtomicUsize);

#[async_trait::async_trait]
impl LlmClient for NoDispatch {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("an idle control request must not dispatch a model")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("an idle control request must not dispatch a model")
    }
}

async fn idle_controls_scenario(store: Arc<dyn SessionStore>, reopen: Option<PathBuf>) {
    for retained_prefix in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(NoDispatch::default());
        let agent = crate::Agent::from_config(crate::agent_api::test_config())
            .await
            .unwrap();
        let id = if retained_prefix {
            "idle-prefix"
        } else {
            "idle-empty"
        };
        let session = agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &crate::SessionOptions::new()
                    .with_session_id(id)
                    .with_session_store(store.clone())
                    .with_auto_save(true),
            )
            .unwrap();
        let thread = crate::fact_control::thread_for_session(id);
        let log_dir = crate::session_folder::effect_log_dir(workspace.path(), id);
        let log = FileLog::open(&log_dir).unwrap();
        if retained_prefix {
            log.append(
                &thread,
                &[NewFact {
                    kind: "user.message".into(),
                    key: "original-retained-input".into(),
                    payload: serde_json::json!({"text": "retained exact input"}),
                }],
                None,
            )
            .unwrap();
        }
        let path = log_dir.join(format!("{thread}.jsonl"));
        let before_bytes = std::fs::read(&path).ok();
        let before_facts = log.read(&thread).unwrap();
        let before_snapshot = serde_json::to_value(store.load_snapshot(id).await.unwrap()).unwrap();

        for request in [
            crate::run_control::SteerRequest::new("new exact input"),
            crate::run_control::SteerRequest::new("foreign exact input").with_run_id("foreign"),
        ] {
            assert!(matches!(
                session.steer(request).await,
                Err(crate::CodeError::RunControl(
                    crate::run_control::RunControlError::NoActiveRun
                ))
            ));
        }
        assert!(matches!(
            session
                .interrupt(crate::run_control::InterruptRequest::new())
                .await,
            Err(crate::CodeError::RunControl(
                crate::run_control::RunControlError::NoActiveRun
            ))
        ));
        assert!(session.current_run().await.is_none());
        assert!(session.run_control_snapshot().await.is_none());
        assert!(session.runs().await.is_empty());
        let fence = session.run_admission.try_acquire(id).unwrap();
        assert!(fence.is_nonexecuting_fence_for(&session.run_admission));
        assert!(
            !session
                .cancel_and_settle(std::time::Duration::ZERO, std::time::Duration::ZERO)
                .await
        );
        assert!(fence.is_nonexecuting_fence_for(&session.run_admission));
        drop(fence);
        assert!(
            session
                .cancel_and_settle(std::time::Duration::ZERO, std::time::Duration::ZERO)
                .await
        );
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
        let scheduler = session.task_scheduler_health().await.unwrap();
        assert_eq!((scheduler.active, scheduler.released), (0, 0));
        assert_eq!(std::fs::read(&path).ok(), before_bytes);
        assert_eq!(
            FileLog::open(&log_dir).unwrap().read(&thread).unwrap(),
            before_facts
        );
        let read: Arc<dyn SessionStore> = match &reopen {
            Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
            None => store.clone(),
        };
        assert_eq!(
            serde_json::to_value(read.load_snapshot(id).await.unwrap()).unwrap(),
            before_snapshot
        );
        session.close().await;
        assert!(matches!(
            session
                .steer(crate::run_control::SteerRequest::new("closed input"))
                .await,
            Err(crate::CodeError::SessionClosed { .. })
        ));
        assert_eq!(std::fs::read(&path).ok(), before_bytes);
    }
}

#[tokio::test]
async fn idle_controls_refuse_without_dispatch_or_fact_writes_in_both_session_stores() {
    idle_controls_scenario(Arc::new(MemorySessionStore::new()), None).await;
    let root = tempfile::tempdir().unwrap();
    idle_controls_scenario(
        Arc::new(FileSessionStore::new(root.path()).await.unwrap()),
        Some(root.path().to_path_buf()),
    )
    .await;
}
