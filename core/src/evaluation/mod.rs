//! Provider-neutral execution facts and auxiliary evaluation primitives.
//!
//! This module is an evaluation substrate, not a product reviewer.  It records
//! bounded execution facts, reads a consistent evidence window, and runs an
//! isolated host-supplied evaluator.  Rubrics, thresholds, findings, and
//! business audit policy stay outside Code Core.

mod auxiliary_run;
mod dispatch_ledger;
mod evidence;
mod file_result_store;
mod identity;
mod journal;
mod protocol;
mod result;
mod supervision;

pub use auxiliary_run::{
    AuxiliaryCapabilityProfileV1, AuxiliaryExecutor, AuxiliaryModeV1, AuxiliaryRunContextV1,
    AuxiliaryRunError, AuxiliaryRunHandle, AuxiliaryRunOutputV1, AuxiliaryRunService,
    AuxiliaryRunSnapshotV1, AuxiliaryRunSpecV1, AuxiliaryRunStateV1, InMemoryAuxiliaryRunService,
    StructuredAuxiliaryExecutor, AUXILIARY_MAX_OUTPUT_BYTES, AUXILIARY_MAX_STEPS,
    AUXILIARY_OUTPUT_SCHEMA_V1, AUXILIARY_RUN_SCHEMA_V1, AUXILIARY_SNAPSHOT_SCHEMA_V1,
};
pub use dispatch_ledger::MemoryEvaluationDispatchLedger as InMemoryEvaluationDispatchLedger;
pub use dispatch_ledger::{
    EvaluationDispatchClaimOutcome, EvaluationDispatchLedger, EvaluationDispatchLedgerError,
    FileEvaluationDispatchLedger, MemoryEvaluationDispatchLedger,
    EVALUATION_DISPATCH_LEASE_GRACE_MS, EVALUATION_DISPATCH_LEDGER_DEFAULT_MAX_RECORDS,
    EVALUATION_DISPATCH_LEDGER_MAX_BYTES, EVALUATION_DISPATCH_LEDGER_SCHEMA_V1,
    EVALUATION_DISPATCH_MIN_LEASE_MS,
};
pub use evidence::{
    EvidenceArtifactV1, EvidenceContentModeV1, EvidenceError, EvidenceEventV1, EvidenceLimitsV1,
    EvidenceReadRequestV1, EvidenceReader, EvidenceRunStateV1, EvidenceSnapshotV1,
    RunEvidenceReader, EVIDENCE_MAX_ARTIFACTS, EVIDENCE_MAX_ARTIFACT_BYTES, EVIDENCE_MAX_EVENTS,
    EVIDENCE_MAX_EVENT_BYTES, EVIDENCE_MAX_PROMPT_BYTES, EVIDENCE_MAX_RESULT_BYTES,
    EVIDENCE_SNAPSHOT_SCHEMA_V1,
};
pub use file_result_store::{
    FileEvaluationResultStore, EVALUATION_RESULT_STORE_DEFAULT_MAX_RECORDS,
    EVALUATION_RESULT_STORE_MAX_BYTES, EVALUATION_RESULT_STORE_SCHEMA_V1,
};
pub use identity::{
    digest_bytes, digest_json, validate_digest, EventCursorV1, ExecutionFrameV1, ExecutionTargetV1,
    IdentityError, EVALUATION_MAX_ID_BYTES, EXECUTION_FRAME_SCHEMA_V1, EXECUTION_TARGET_SCHEMA_V1,
};
pub use journal::{
    ExecutionFactInputV1, ExecutionFactJournal, ExecutionFactKindV1, ExecutionFactPageV1,
    ExecutionFactRecorder, ExecutionFactSnapshotV1, ExecutionFactV1, FactAppendOutcomeV1,
    InMemoryExecutionFactJournal, JournalError, EXECUTION_FACT_SCHEMA_V1,
};
pub use protocol::{
    EvaluationProtocolError, EvaluationWireEnvelopeV1, EvaluationWireKindDescriptorV1,
    EvaluationWireKindV1, EvaluationWireTypeV1, EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES,
    EVALUATION_PROTOCOL_SCHEMA_V1, EVALUATION_PROTOCOL_VERSION_V1,
    EVALUATION_WIRE_KIND_DESCRIPTORS_V1,
};
pub use result::{
    EvaluationRecordV1, EvaluationResultSink, EvaluationResultV1, EvaluationStoreError,
    EvaluationWriteOutcomeV1, InMemoryEvaluationResultStore, EVALUATION_RECORD_SCHEMA_V1,
    EVALUATION_RESULT_SCHEMA_V1,
};
pub use supervision::{
    EvaluationBoundaryV1, EvaluationDispatch, EvaluationDispatchOutcome, EvaluationPlanV1,
    EvaluationPolicy, EvaluationSupervisor, SupervisorError, EVALUATION_MAX_COOLDOWN_MS,
    EVALUATION_MAX_PENDING, EVALUATION_PLAN_SCHEMA_V1,
};
