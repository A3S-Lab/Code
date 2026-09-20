//! S-PW-01: fifteen resumable workflows lose only the in-flight second step.

use super::*;
use crate::store::MemorySessionStore;

const WORKFLOWS: usize = 15;

struct CountingExecutor {
    ran: Arc<tokio::sync::Mutex<Vec<(String, bool)>>>,
    fail_second_step: bool,
}

#[async_trait::async_trait]
impl AgentExecutor for CountingExecutor {
    async fn execute_step(
        &self,
        spec: AgentStepSpec,
        _event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> StepOutcome {
        let failed = self.fail_second_step && spec.task_id.ends_with("-step2");
        self.ran.lock().await.push((spec.task_id.clone(), !failed));
        StepOutcome {
            task_id: spec.task_id,
            session_id: "task-run".into(),
            agent: spec.agent,
            output: if failed {
                "killed-in-flight".into()
            } else {
                "done".into()
            },
            success: !failed,
            structured: None,
            source_anchors: Vec::new(),
        }
    }

    fn concurrency_hint(&self) -> usize {
        1
    }
}

fn specs(index: usize) -> Vec<AgentStepSpec> {
    vec![
        AgentStepSpec::new(format!("w{index}-step1"), "explore", "d", "step-1"),
        AgentStepSpec::new(format!("w{index}-step2"), "review", "d", "step-2"),
    ]
}

#[tokio::test]
#[ignore = "S-PW-01 soak: 15 workflows resume after step 2 is lost in flight"]
async fn soak_resume_does_not_repeat_completed_steps() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let ran = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let first = Arc::new(CountingExecutor {
        ran: Arc::clone(&ran),
        fail_second_step: true,
    });
    for index in 0..WORKFLOWS {
        let workflow_id = format!("wf-{index}");
        let out = execute_steps_parallel_resumable(
            first.clone(),
            specs(index),
            &workflow_id,
            Arc::clone(&store),
            None,
        )
        .await;
        assert!(out[0].success, "step 1 journals before the in-flight loss");
        assert!(!out[1].success, "step 2 is the killed in-flight step");
    }

    let second = Arc::new(CountingExecutor {
        ran: Arc::clone(&ran),
        fail_second_step: false,
    });
    for index in 0..WORKFLOWS {
        let workflow_id = format!("wf-{index}");
        let out = execute_steps_parallel_resumable(
            second.clone(),
            specs(index),
            &workflow_id,
            Arc::clone(&store),
            None,
        )
        .await;
        assert!(out.iter().all(|outcome| outcome.success));
        assert_eq!(out[0].output, "done");
        assert_eq!(out[1].output, "done");
    }

    let runs = ran.lock().await;
    let step1 = runs.iter().filter(|(id, _)| id.ends_with("-step1")).count();
    let step2_ok = runs
        .iter()
        .filter(|(id, ok)| id.ends_with("-step2") && *ok)
        .count();
    assert_eq!(step1, WORKFLOWS, "step 1 must not run again on resume");
    assert_eq!(step2_ok, WORKFLOWS, "step 2 succeeds once after resume");

    let mut done = HashMap::new();
    done.insert(
        "w0-step1".to_string(),
        StepOutcome {
            task_id: "w0-step1".into(),
            session_id: "task-run".into(),
            agent: "explore".into(),
            output: "cached".into(),
            success: true,
            structured: None,
            source_anchors: Vec::new(),
        },
    );
    let mut checkpoint = WorkflowCheckpoint::from_completed("wf-missing", &done, 1);
    checkpoint.schema_version = crate::orchestration::WORKFLOW_CHECKPOINT_SCHEMA_VERSION + 1;
    store
        .save_workflow_checkpoint("wf-missing", &checkpoint)
        .await
        .unwrap();
    let before = runs.len();
    drop(runs);
    let out = execute_steps_parallel_resumable(second, specs(0), "wf-missing", store, None).await;
    assert!(out.iter().all(|outcome| !outcome.success));
    assert!(out
        .iter()
        .all(|outcome| outcome.output.contains("cannot be resumed")));
    assert_eq!(
        ran.lock().await.len(),
        before,
        "an unreadable checkpoint must not restart completed work"
    );
}
