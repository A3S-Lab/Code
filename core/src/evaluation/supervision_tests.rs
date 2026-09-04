use super::*;
use crate::agent::AgentEvent;
use crate::evaluation::auxiliary_run::{
    AuxiliaryExecutor, AuxiliaryRunContextV1, InMemoryAuxiliaryRunService,
};
use crate::evaluation::evidence::RunEvidenceReader;
use crate::evaluation::evidence::{EvidenceError, EvidenceReadRequestV1};
use crate::evaluation::identity::ExecutionTargetV1;
use crate::evaluation::journal::InMemoryExecutionFactJournal;
use crate::run::InMemoryRunStore;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

struct TurnPolicy;

impl EvaluationPolicy for TurnPolicy {
    fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
        Some(EvaluationPlanV1::new(
            EvaluationBoundaryV1::TurnEnd,
            "turn-check",
            "inspect the bounded evidence",
        ))
    }
}

struct RecordingExecutor;

#[async_trait]
impl AuxiliaryExecutor for RecordingExecutor {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        Ok(serde_json::json!({
            "sequence": context.evidence.events.first().map(|event| event.sequence)
        }))
    }
}

#[tokio::test]
async fn policy_is_boundary_and_replay_safe() {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("run-1".into(), "session-1", "prompt")
        .await;
    let target = ExecutionTargetV1::new("session-1", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    let service = Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
        RecordingExecutor,
    )));
    let supervisor = EvaluationSupervisor::new(
        journal,
        Arc::new(RunEvidenceReader::new(Arc::clone(&runs))),
        service,
        Arc::new(TurnPolicy),
    );
    let start = RunEventRecord {
        sequence: 0,
        timestamp_ms: 1,
        event: AgentEvent::TurnStart { turn: 1 },
    };
    let ignored = supervisor
        .observe_event(ExecutionFrameV1::root(target.clone()), &start)
        .await
        .unwrap();
    assert_eq!(ignored.outcome, EvaluationDispatchOutcome::Ignored);
    let end = RunEventRecord {
        sequence: 1,
        timestamp_ms: 2,
        event: AgentEvent::TurnEnd {
            turn: 1,
            usage: crate::llm::TokenUsage::default(),
        },
    };
    let dispatched = supervisor
        .observe_event(ExecutionFrameV1::root(target.clone()), &end)
        .await
        .unwrap();
    assert_eq!(dispatched.outcome, EvaluationDispatchOutcome::Dispatched);
    dispatched.handle.unwrap().wait().await.unwrap();
    let replay = supervisor
        .observe_event(ExecutionFrameV1::root(target), &end)
        .await
        .unwrap();
    assert_eq!(replay.outcome, EvaluationDispatchOutcome::Ignored);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_replay_cannot_dispatch_the_same_fact_twice() {
    struct ConcurrentEveryEvent;
    impl EvaluationPolicy for ConcurrentEveryEvent {
        fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
            let mut plan = EvaluationPlanV1::new(
                EvaluationBoundaryV1::EveryEvent,
                "concurrent-replay",
                "inspect",
            );
            plan.max_pending = 8;
            Some(plan)
        }
    }

    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("concurrent-run".into(), "concurrent-session", "prompt")
        .await;
    let target = ExecutionTargetV1::new("concurrent-session", &run.id);
    let record = RunEventRecord {
        sequence: 0,
        timestamp_ms: 1,
        event: AgentEvent::TextDelta {
            text: "same fact".into(),
        },
    };
    runs.record_event(&run.id, record.event.clone()).await;
    let service = Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
        RecordingExecutor,
    )));
    let supervisor = Arc::new(EvaluationSupervisor::new(
        Arc::new(InMemoryExecutionFactJournal::new()),
        Arc::new(RunEvidenceReader::new(runs)),
        service.clone(),
        Arc::new(ConcurrentEveryEvent),
    ));

    let tasks = (0..8).map(|_| {
        let supervisor = Arc::clone(&supervisor);
        let target = target.clone();
        let record = record.clone();
        tokio::spawn(async move {
            supervisor
                .observe_event(ExecutionFrameV1::root(target), &record)
                .await
        })
    });
    let results = futures::future::join_all(tasks).await;
    let mut dispatched = 0;
    let mut ignored = 0;
    let mut handles = Vec::new();
    for result in results {
        let dispatch = result.unwrap().unwrap();
        match dispatch.outcome {
            EvaluationDispatchOutcome::Dispatched => {
                dispatched += 1;
                handles.push(dispatch.handle.unwrap());
            }
            EvaluationDispatchOutcome::Ignored => ignored += 1,
            EvaluationDispatchOutcome::Suppressed => {
                panic!("concurrent replay unexpectedly hit a capacity suppression")
            }
        }
    }
    assert_eq!(dispatched, 1);
    assert_eq!(ignored, 7);
    for handle in handles {
        handle.wait().await.unwrap();
    }
    assert_eq!(service.list().await.len(), 1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn pending_cap_suppresses_without_blocking_parent() {
    struct SlowExecutor;
    #[async_trait]
    impl AuxiliaryExecutor for SlowExecutor {
        async fn execute(
            &self,
            context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            context.cancellation.cancelled().await;
            Err(AuxiliaryRunError::Cancelled)
        }
    }
    struct EveryEvent;
    impl EvaluationPolicy for EveryEvent {
        fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
            Some(EvaluationPlanV1::new(
                EvaluationBoundaryV1::EveryEvent,
                "one-at-a-time",
                "wait",
            ))
        }
    }
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("run-2".into(), "session-2", "prompt")
        .await;
    let target = ExecutionTargetV1::new("session-2", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    let service = Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(SlowExecutor)));
    let supervisor = EvaluationSupervisor::new(
        journal,
        Arc::new(RunEvidenceReader::new(runs)),
        service,
        Arc::new(EveryEvent),
    );
    let event = |sequence| RunEventRecord {
        sequence,
        timestamp_ms: (sequence + 1) as u64,
        event: AgentEvent::TextDelta {
            text: format!("event-{sequence}"),
        },
    };
    let first = supervisor
        .observe_event(ExecutionFrameV1::root(target.clone()), &event(0))
        .await
        .unwrap();
    assert_eq!(first.outcome, EvaluationDispatchOutcome::Dispatched);
    let second = supervisor
        .observe_event(ExecutionFrameV1::root(target), &event(1))
        .await
        .unwrap();
    assert_eq!(second.outcome, EvaluationDispatchOutcome::Suppressed);
    assert_eq!(supervisor.pending_count().await, 1);
    first.handle.unwrap().cancel().await;
    supervisor.shutdown().await;
    assert_eq!(supervisor.pending_count().await, 0);
}

#[tokio::test]
async fn failed_evidence_admission_can_retry_an_exact_replay() {
    struct FlakyReader {
        calls: AtomicUsize,
        inner: RunEvidenceReader,
    }

    #[async_trait]
    impl EvidenceReader for FlakyReader {
        async fn read(
            &self,
            request: EvidenceReadRequestV1,
        ) -> Result<super::super::evidence::EvidenceSnapshotV1, EvidenceError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(EvidenceError::RunNotFound);
            }
            self.inner.read(request).await
        }
    }

    struct EveryEvent;
    impl EvaluationPolicy for EveryEvent {
        fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
            Some(
                EvaluationPlanV1::new(EvaluationBoundaryV1::EveryEvent, "retryable", "inspect")
                    .with_cooldown_ms(60_000),
            )
        }
    }

    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("retry-run".into(), "retry-session", "prompt")
        .await;
    let target = ExecutionTargetV1::new("retry-session", &run.id);
    let record = RunEventRecord {
        sequence: 0,
        timestamp_ms: 1,
        event: AgentEvent::TextDelta {
            text: "event".into(),
        },
    };
    runs.record_event(&run.id, record.event.clone()).await;
    let supervisor = EvaluationSupervisor::new(
        Arc::new(InMemoryExecutionFactJournal::new()),
        Arc::new(FlakyReader {
            calls: AtomicUsize::new(0),
            inner: RunEvidenceReader::new(Arc::clone(&runs)),
        }),
        Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
            RecordingExecutor,
        ))),
        Arc::new(EveryEvent),
    );
    assert!(matches!(
        supervisor
            .observe_event(ExecutionFrameV1::root(target.clone()), &record)
            .await,
        Err(SupervisorError::Evidence(_))
    ));
    let retry = supervisor
        .observe_event(ExecutionFrameV1::root(target), &record)
        .await
        .unwrap();
    assert_eq!(retry.outcome, EvaluationDispatchOutcome::Dispatched);
    retry.handle.unwrap().wait().await.unwrap();
}

#[tokio::test]
async fn cancelled_evidence_admission_releases_reservation_synchronously() {
    struct BlockingReader {
        calls: AtomicUsize,
        started: Notify,
        release: Notify,
        inner: RunEvidenceReader,
    }

    #[async_trait]
    impl EvidenceReader for BlockingReader {
        async fn read(
            &self,
            request: EvidenceReadRequestV1,
        ) -> Result<super::super::evidence::EvidenceSnapshotV1, EvidenceError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            self.inner.read(request).await
        }
    }

    struct EveryEvent;
    impl EvaluationPolicy for EveryEvent {
        fn plan(&self, _fact: &ExecutionFactV1) -> Option<EvaluationPlanV1> {
            Some(EvaluationPlanV1::new(
                EvaluationBoundaryV1::EveryEvent,
                "cancel-retry",
                "inspect",
            ))
        }
    }

    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("cancel-run".into(), "cancel-session", "prompt")
        .await;
    let record = RunEventRecord {
        sequence: 0,
        timestamp_ms: 1,
        event: AgentEvent::TextDelta {
            text: "event".into(),
        },
    };
    runs.record_event(&run.id, record.event.clone()).await;
    let target = ExecutionTargetV1::new("cancel-session", &run.id);
    let reader = Arc::new(BlockingReader {
        calls: AtomicUsize::new(0),
        started: Notify::new(),
        release: Notify::new(),
        inner: RunEvidenceReader::new(Arc::clone(&runs)),
    });
    let supervisor = Arc::new(EvaluationSupervisor::new(
        Arc::new(InMemoryExecutionFactJournal::new()),
        reader.clone(),
        Arc::new(InMemoryAuxiliaryRunService::new(Arc::new(
            RecordingExecutor,
        ))),
        Arc::new(EveryEvent),
    ));
    let task_supervisor = Arc::clone(&supervisor);
    let task_target = target.clone();
    let task_record = record.clone();
    let task = tokio::spawn(async move {
        task_supervisor
            .observe_event(ExecutionFrameV1::root(task_target), &task_record)
            .await
    });
    reader.started.notified().await;
    task.abort();
    assert!(task.await.is_err());
    assert_eq!(supervisor.pending_count().await, 0);

    let retry = supervisor
        .observe_event(ExecutionFrameV1::root(target), &record)
        .await
        .unwrap();
    assert_eq!(retry.outcome, EvaluationDispatchOutcome::Dispatched);
    retry.handle.unwrap().wait().await.unwrap();
}
