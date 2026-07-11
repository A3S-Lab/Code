use super::{GraphPatch, ObjectId, RelationId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GRAPH_EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GraphEvent {
    GoalCreated {
        goal: String,
    },
    ObjectCreated {
        id: ObjectId,
        object_type: String,
        data: Value,
    },
    ObjectUpdated {
        id: ObjectId,
        version: u64,
        data: Value,
    },
    ObjectRemoved {
        id: ObjectId,
        version: u64,
    },
    RelationCreated {
        id: RelationId,
        relation_type: String,
        source: ObjectId,
        target: ObjectId,
        data: Value,
    },
    RelationUpdated {
        id: RelationId,
        version: u64,
        data: Value,
    },
    RelationRemoved {
        id: RelationId,
        version: u64,
    },
    PatchProposed {
        patch_id: String,
        patch: GraphPatch,
    },
    PatchApplied {
        patch_id: String,
    },
    PatchRejected {
        patch_id: String,
        reason: String,
    },
    BehaviorStarted {
        name: String,
    },
    BehaviorCompleted {
        name: String,
    },
    BehaviorFailed {
        name: String,
        error: String,
    },
    BranchForked {
        parent_branch_id: String,
        fork_sequence: u64,
    },
    /// Audit fact imported from another authoritative event stream.
    ExternalEventObserved {
        source: String,
        stream_id: String,
        sequence: u64,
        event_id: String,
        name: String,
        payload: Value,
    },
    Custom {
        name: String,
        payload: Value,
    },
}

impl GraphEvent {
    pub fn event_type(&self) -> &str {
        match self {
            Self::GoalCreated { .. } => "goal.created",
            Self::ObjectCreated { .. } => "object.created",
            Self::ObjectUpdated { .. } => "object.updated",
            Self::ObjectRemoved { .. } => "object.removed",
            Self::RelationCreated { .. } => "relation.created",
            Self::RelationUpdated { .. } => "relation.updated",
            Self::RelationRemoved { .. } => "relation.removed",
            Self::PatchProposed { .. } => "patch.proposed",
            Self::PatchApplied { .. } => "patch.applied",
            Self::PatchRejected { .. } => "patch.rejected",
            Self::BehaviorStarted { .. } => "behavior.started",
            Self::BehaviorCompleted { .. } => "behavior.completed",
            Self::BehaviorFailed { .. } => "behavior.failed",
            Self::BranchForked { .. } => "branch.forked",
            Self::ExternalEventObserved { name, .. } => name,
            Self::Custom { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEventRecord {
    pub schema_version: u32,
    pub id: String,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub branch_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub state_version_before: u64,
    pub state_version_after: u64,
    /// SHA-256 of the deterministic graph projection after this event.
    pub state_hash_after: String,
    /// Hash of the preceding record, forming a tamper-evident event chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_record_hash: Option<String>,
    /// SHA-256 over this record's immutable fields and `previous_record_hash`.
    pub record_hash: String,
    pub event: GraphEvent,
}
