//! Event-sourced reactive state graph for auditable agent workflows.
//!
//! The append-only [`GraphEventRecord`] stream is the source of truth. The
//! in-memory [`StateGraph`] is only a projection and can always be rebuilt with
//! [`GraphRuntime::strict_replay`].

mod behavior;
mod event;
mod graph;
mod runtime;
mod store;
mod types;

pub use behavior::{Behavior, BehaviorContext, BehaviorError, EventFilter, FnBehavior};
pub use event::{GraphEvent, GraphEventRecord, GRAPH_EVENT_SCHEMA_VERSION};
pub use graph::{GraphDiff, ReplayError, StateGraph};
pub use runtime::{
    ExternalEvent, ExternalProjectionOutcome, GraphRuntime, RuntimeError, RuntimeLimits,
};
pub use store::{
    graph_event_head, FileGraphEventStore, GraphEventStore, GraphSaveOutcome, MemoryGraphEventStore,
};
pub use types::{GraphObject, GraphPatch, GraphRelation, ObjectId, PatchOperation, RelationId};

#[cfg(test)]
mod tests;
