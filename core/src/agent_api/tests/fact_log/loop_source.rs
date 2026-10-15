#![cfg(test)]

use super::*;
use crate::session_checkpoint::{
    SessionCheckpointExportSink, SessionCheckpointExportV1, SessionLogicalResumeEvidenceV1,
};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};

#[derive(Default)]
struct Exports(std::sync::Mutex<Vec<SessionCheckpointExportV1>>);

#[async_trait]
impl SessionCheckpointExportSink for Exports {
    async fn export_checkpoint(&self, value: SessionCheckpointExportV1) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(value);
        Ok(())
    }
}

async fn prove_recovery(file: bool) {
    for policy in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("note.txt"), "original evidence").unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            Arc::new(MemorySessionStore::new())
        };
        let exports = Arc::new(Exports::default());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = Arc::new(ScriptedClient {
            seen: seen.clone(),
            responses: std::sync::Mutex::new(vec![
                tool_response(
                    "read-source",
                    "read",
                    serde_json::json!({"file_path":"note.txt"}),
                ),
                text_response("original evidenced answer"),
            ]),
        });
        let agent = super::durable_source::source_agent().await;
        let mut options = crate::SessionOptions::new()
            .with_session_id("loop-source")
            .with_session_store(store.clone())
            .with_llm_client(client.clone())
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
            .with_planning(false)
            .with_auto_delegation_enabled(false)
            .with_manual_delegation_enabled(false)
            .with_continuation(false);
        if policy {
            options = options.with_work_policy(
                crate::WorkPolicyV1::new("loop-source", 1, "Preserve original evidence.").unwrap(),
            );
        }
        let session = agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &options,
            )
            .unwrap();
        session
            .set_session_checkpoint_export_sink(Some(exports.clone()))
            .unwrap();
        session.send("Read note.txt", None).await.unwrap();
        session.save().await.unwrap();
        let payload = exports
            .0
            .lock()
            .unwrap()
            .last()
            .expect("original committed export")
            .open()
            .unwrap();
        let checkpoint = payload.logical_resume.unwrap();
        assert_eq!(checkpoint.schema_version, if policy { 2 } else { 1 });
        assert_eq!(
            checkpoint.durable_source,
            payload.snapshot.session.durable_source
        );
        assert_eq!(checkpoint.tool_calls_count, 1);
        let original = read_workspace_facts(workspace.path(), "loop-source").unwrap();
        session.close().await;
        agent.close().await;
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            store
        };
        store
            .save_loop_checkpoint(&checkpoint.run_id, &checkpoint)
            .await
            .unwrap();
        options.work_policy = None;
        options.session_store = Some(store.clone());
        let agent = super::durable_source::source_agent().await;
        let session = agent
            .resume_session_async("loop-source", options)
            .await
            .unwrap();
        let answer = session.resume_run(&checkpoint.run_id).await.unwrap();
        assert_eq!(answer.text, "original evidenced answer");
        for prepared in [false, true] {
            let target = if prepared {
                "prepared-recovery"
            } else {
                "exact-recovery"
            };
            let spawned = if prepared {
                let evidence =
                    SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
                let admission = session
                    .prepare_recovery_from_checkpoint(
                        &evidence,
                        "original-export",
                        target,
                        checkpoint.clone(),
                    )
                    .await
                    .unwrap();
                let crate::agent_api::ExactRecoveryPreparation::Ready(admission) = admission else {
                    panic!("new target must be admitted")
                };
                session.spawn_prepared_recovery(*admission).await.unwrap()
            } else {
                session
                    .spawn_recovery_with_run_id(&checkpoint.run_id, target)
                    .await
                    .unwrap()
            };
            let crate::agent_api::AgentRunSpawn::Started { worker, .. } = spawned else {
                panic!("new target must start")
            };
            tokio::time::timeout(std::time::Duration::from_secs(10), worker)
                .await
                .unwrap()
                .unwrap();
            let result = session.run_snapshot(target).await.unwrap();
            assert_eq!(result.status, crate::run::RunStatus::Completed);
            assert_eq!(
                result.result_text.as_deref(),
                Some("original evidenced answer")
            );
        }
        let after = read_workspace_facts(workspace.path(), "loop-source").unwrap();
        assert_eq!(&after[..original.len()], original.as_slice());
        assert_eq!(
            after
                .iter()
                .filter(|fact| fact.kind == "tool.result")
                .count(),
            1,
            "no repeated original tool"
        );
        session.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn durable_loop_original_exports_resume_all_ingresses_memory_store() {
    prove_recovery(false).await;
}

#[tokio::test]
async fn durable_loop_original_exports_resume_all_ingresses_file_reopen() {
    prove_recovery(true).await;
}
