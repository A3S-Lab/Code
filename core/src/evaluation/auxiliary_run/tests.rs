use super::*;
use crate::evaluation::evidence::{EvidenceReadRequestV1, RunEvidenceReader};
use crate::evaluation::identity::{digest_bytes, ExecutionTargetV1};
use crate::run::InMemoryRunStore;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FixtureExecutor {
    calls: AtomicUsize,
    value: serde_json::Value,
}

#[async_trait]
impl AuxiliaryExecutor for FixtureExecutor {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!context.spec.capabilities.write_workspace);
        Ok(self.value.clone())
    }
}

async fn evidence() -> EvidenceSnapshotV1 {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("run-1".into(), "session-1", "prompt")
        .await;
    RunEvidenceReader::new(runs)
        .read(EvidenceReadRequestV1::new(ExecutionTargetV1::new(
            "session-1",
            &run.id,
        )))
        .await
        .unwrap()
}

#[tokio::test]
async fn service_runs_isolated_executor_and_is_idempotent() {
    let evidence = evidence().await;
    let executor = Arc::new(FixtureExecutor {
        calls: AtomicUsize::new(0),
        value: serde_json::json!({"decision": "ok"}),
    });
    let service = InMemoryAuxiliaryRunService::new(executor.clone());
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(evidence.target.clone()),
        "fixture",
        "inspect bounded evidence",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-1")
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let handle = service
        .spawn(spec.clone(), evidence.clone(), None)
        .await
        .unwrap();
    let replay = service.spawn(spec, evidence, None).await.unwrap();
    assert_eq!(handle.id(), replay.id());
    let output = handle.wait().await.unwrap();
    assert_eq!(output.value["decision"], "ok");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.get("aux-1").await.unwrap().state,
        AuxiliaryRunStateV1::Completed
    );
}

#[tokio::test]
async fn parent_cancellation_is_propagated() {
    struct BlockingExecutor;
    #[async_trait]
    impl AuxiliaryExecutor for BlockingExecutor {
        async fn execute(
            &self,
            context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            context.cancellation.cancelled().await;
            Err(AuxiliaryRunError::Cancelled)
        }
    }
    let evidence = evidence().await;
    let service = InMemoryAuxiliaryRunService::new(Arc::new(BlockingExecutor));
    let parent = CancellationToken::new();
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(evidence.target.clone()),
        "cancel-fixture",
        "wait",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-cancel");
    let handle = service
        .spawn(spec, evidence, Some(parent.clone()))
        .await
        .unwrap();
    parent.cancel();
    assert!(matches!(
        handle.wait().await,
        Err(AuxiliaryRunError::Cancelled)
    ));
    assert_eq!(
        handle.snapshot().await.state,
        AuxiliaryRunStateV1::Cancelled
    );
}

#[test]
fn capability_ceiling_rejects_escalation() {
    let target = ExecutionTargetV1::new("s", "r");
    let frame = ExecutionFrameV1::root(target);
    let digest = digest_bytes("evidence", b"x");
    let spec = AuxiliaryRunSpecV1::new(frame, "x", "y", digest)
        .with_capabilities(AuxiliaryCapabilityProfileV1::read_only(1024))
        .with_parent_ceiling(AuxiliaryCapabilityProfileV1::tool_free());
    assert!(matches!(
        spec.validate(&spec.evidence_digest),
        Err(AuxiliaryRunError::CapabilityEscalation)
    ));
}

#[tokio::test]
async fn output_schema_and_limit_are_enforced() {
    let evidence = evidence().await;
    let service = InMemoryAuxiliaryRunService::new(Arc::new(FixtureExecutor {
        calls: AtomicUsize::new(0),
        value: serde_json::json!({"wrong": true}),
    }));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(evidence.target.clone()),
        "schema-fixture",
        "return object",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-schema")
    .with_output_schema(serde_json::json!({
        "type": "object",
        "required": ["answer"],
        "properties": {"answer": {"type": "string"}}
    }));
    let handle = service.spawn(spec, evidence, None).await.unwrap();
    assert!(matches!(
        handle.wait().await,
        Err(AuxiliaryRunError::OutputSchemaMismatch)
    ));
}

#[tokio::test]
async fn service_rejects_cross_target_evidence() {
    let evidence = evidence().await;
    let other_target = ExecutionTargetV1::new("session-other", "run-other");
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(other_target),
        "cross-target",
        "must be rejected",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-cross-target");
    let service = InMemoryAuxiliaryRunService::new(Arc::new(FixtureExecutor {
        calls: AtomicUsize::new(0),
        value: serde_json::json!({}),
    }));
    assert!(matches!(
        service.spawn(spec, evidence, None).await,
        Err(AuxiliaryRunError::TargetMismatch)
    ));
}

#[tokio::test]
async fn executor_panics_and_output_overflow_become_terminal_failures() {
    struct PanicExecutor;
    #[async_trait]
    impl AuxiliaryExecutor for PanicExecutor {
        async fn execute(
            &self,
            _context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            panic!("fixture panic");
        }
    }

    let evidence = evidence().await;
    let service = InMemoryAuxiliaryRunService::new(Arc::new(PanicExecutor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(evidence.target.clone()),
        "panic",
        "panic",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-panic");
    let handle = service.spawn(spec, evidence.clone(), None).await.unwrap();
    assert!(matches!(
        handle.wait().await,
        Err(AuxiliaryRunError::Executor(message)) if message.contains("panicked")
    ));
    assert_eq!(handle.snapshot().await.state, AuxiliaryRunStateV1::Failed);

    struct LargeExecutor;
    #[async_trait]
    impl AuxiliaryExecutor for LargeExecutor {
        async fn execute(
            &self,
            _context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            Ok(serde_json::json!("too large"))
        }
    }
    let service = InMemoryAuxiliaryRunService::new(Arc::new(LargeExecutor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(evidence.target.clone()),
        "overflow",
        "overflow",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-overflow")
    .with_capabilities(AuxiliaryCapabilityProfileV1::read_only(1));
    let handle = service.spawn(spec, evidence, None).await.unwrap();
    assert!(matches!(
        handle.wait().await,
        Err(AuxiliaryRunError::OutputLimit)
    ));
    assert_eq!(handle.snapshot().await.state, AuxiliaryRunStateV1::Failed);
}
