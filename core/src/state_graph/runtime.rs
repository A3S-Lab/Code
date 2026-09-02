use super::behavior::{Behavior, BehaviorContext};
use super::graph::{record_hash, replay_strict, StructuralHash};
use super::{
    GraphDiff, GraphEvent, GraphEventRecord, GraphObject, GraphPatch, GraphRelation,
    PatchOperation, ReplayError, StateGraph, GRAPH_EVENT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    pub max_events: usize,
    pub max_behavior_depth: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_events: 10_000,
            max_behavior_depth: 64,
        }
    }
}

pub struct GraphRuntime {
    branch_id: String,
    correlation_id: Option<String>,
    graph: StateGraph,
    structural_hash: StructuralHash,
    events: Vec<GraphEventRecord>,
    behaviors: Vec<Arc<dyn Behavior>>,
    pending: VecDeque<(usize, GraphEventRecord)>,
    limits: RuntimeLimits,
    external_cursors: BTreeMap<(String, String), ExternalCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalCursor {
    sequence: u64,
    event_id: String,
    observed: BTreeMap<u64, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalProjectionOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalEvent {
    pub source: String,
    pub stream_id: String,
    pub sequence: u64,
    pub event_id: String,
    pub name: String,
    pub payload: serde_json::Value,
}

impl GraphRuntime {
    pub fn new() -> Self {
        Self::with_limits(RuntimeLimits::default())
    }

    pub fn with_limits(limits: RuntimeLimits) -> Self {
        Self {
            branch_id: new_id("branch"),
            correlation_id: None,
            graph: StateGraph::default(),
            structural_hash: StructuralHash::default(),
            events: Vec::new(),
            behaviors: Vec::new(),
            pending: VecDeque::new(),
            limits,
            external_cursors: BTreeMap::new(),
        }
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    pub fn restore(events: Vec<GraphEventRecord>) -> Result<Self, ReplayError> {
        let graph = replay_strict(&events)?;
        let structural_hash = StructuralHash::from_graph(&graph)?;
        let external_cursors = external_cursors(&events)?;
        let branch_id = events
            .last()
            .map(|record| record.branch_id.clone())
            .unwrap_or_else(|| new_id("branch"));
        let correlation_id = events
            .iter()
            .rev()
            .find_map(|record| record.correlation_id.clone());
        Ok(Self {
            branch_id,
            correlation_id,
            graph,
            structural_hash,
            events,
            behaviors: Vec::new(),
            pending: VecDeque::new(),
            limits: RuntimeLimits::default(),
            external_cursors,
        })
    }

    pub fn branch_id(&self) -> &str {
        &self.branch_id
    }

    pub fn graph(&self) -> &StateGraph {
        &self.graph
    }

    pub fn events(&self) -> &[GraphEventRecord] {
        &self.events
    }

    pub fn register(&mut self, behavior: Arc<dyn Behavior>) -> Result<(), RuntimeError> {
        if self
            .behaviors
            .iter()
            .any(|registered| registered.name() == behavior.name())
        {
            return Err(RuntimeError::DuplicateBehavior(behavior.name().to_string()));
        }
        self.behaviors.push(behavior);
        Ok(())
    }

    pub fn emit(&mut self, event: GraphEvent) -> Result<GraphEventRecord, RuntimeError> {
        let record = self.append(event, None)?;
        self.pending.push_back((0, record.clone()));
        self.drain_behaviors()?;
        Ok(record)
    }

    pub fn run_goal(&mut self, goal: impl Into<String>) -> Result<GraphEventRecord, RuntimeError> {
        self.emit(GraphEvent::GoalCreated { goal: goal.into() })
    }

    pub fn propose_patch(
        &mut self,
        patch: GraphPatch,
        causation_id: Option<String>,
    ) -> Result<bool, RuntimeError> {
        let records = self.apply_patch(patch, causation_id, 0)?;
        self.drain_behaviors()?;
        Ok(records)
    }

    pub fn fork_at(&self, sequence_exclusive: u64) -> Result<Self, RuntimeError> {
        let end = usize::try_from(sequence_exclusive)
            .map_err(|_| RuntimeError::InvalidFork(sequence_exclusive))?;
        if end > self.events.len() {
            return Err(RuntimeError::InvalidFork(sequence_exclusive));
        }
        let events = self.events[..end].to_vec();
        let graph = replay_strict(&events)?;
        let structural_hash = StructuralHash::from_graph(&graph)?;
        let external_cursors = external_cursors(&events)?;
        let parent_branch_id = self.branch_id.clone();
        let mut fork = Self {
            branch_id: new_id("branch"),
            correlation_id: self.correlation_id.clone(),
            graph,
            structural_hash,
            events,
            behaviors: self.behaviors.clone(),
            pending: VecDeque::new(),
            limits: self.limits,
            external_cursors,
        };
        let cause = fork.events.last().map(|record| record.id.clone());
        fork.append(
            GraphEvent::BranchForked {
                parent_branch_id,
                fork_sequence: sequence_exclusive,
            },
            cause,
        )?;
        Ok(fork)
    }

    pub fn diff(&self, other: &Self) -> GraphDiff {
        self.graph.diff(&other.graph)
    }

    pub fn strict_replay(records: &[GraphEventRecord]) -> Result<StateGraph, ReplayError> {
        external_cursors(records)?;
        replay_strict(records)
    }

    /// Validate sequence and idempotency before constructing a projection.
    pub fn check_external(
        &self,
        event: &ExternalEvent,
    ) -> Result<Option<ExternalProjectionOutcome>, RuntimeError> {
        let key = (event.source.clone(), event.stream_id.clone());
        let expected = self
            .external_cursors
            .get(&key)
            .map_or(1, |cursor| cursor.sequence.saturating_add(1));
        if let Some(cursor) = self.external_cursors.get(&key) {
            if event.sequence <= cursor.sequence {
                if cursor.observed.get(&event.sequence) == Some(&event.event_id) {
                    return Ok(Some(ExternalProjectionOutcome::Duplicate));
                }
                return Err(RuntimeError::ExternalEventConflict {
                    source_name: event.source.clone(),
                    stream_id: event.stream_id.clone(),
                    sequence: event.sequence,
                });
            }
        }
        if event.sequence != expected {
            return Err(RuntimeError::ExternalSequenceDiverged {
                source_name: event.source.clone(),
                stream_id: event.stream_id.clone(),
                expected,
                actual: event.sequence,
            });
        }
        Ok(None)
    }

    /// Atomically append a graph patch and its durable external-stream cursor.
    pub fn project_external(
        &mut self,
        event: ExternalEvent,
        patch: GraphPatch,
    ) -> Result<ExternalProjectionOutcome, RuntimeError> {
        let key = (event.source.clone(), event.stream_id.clone());
        if let Some(outcome) = self.check_external(&event)? {
            return Ok(outcome);
        }
        let mutation_events = self
            .validate_patch(&patch)
            .map_err(RuntimeError::InvalidExternalProjection)?;
        let required = mutation_events.len().saturating_add(3);
        if self.events.len().saturating_add(required) > self.limits.max_events {
            return Err(RuntimeError::EventLimitExceeded(self.limits.max_events));
        }

        let patch_id = new_id("patch");
        let proposed = self.append(
            GraphEvent::PatchProposed {
                patch_id: patch_id.clone(),
                patch,
            },
            None,
        )?;
        let mut last_cause = proposed.id;
        for mutation in mutation_events {
            let record = self.append(mutation, Some(last_cause))?;
            last_cause = record.id;
        }
        let applied = self.append(GraphEvent::PatchApplied { patch_id }, Some(last_cause))?;
        let observed = self.append(
            GraphEvent::ExternalEventObserved {
                source: event.source.clone(),
                stream_id: event.stream_id.clone(),
                sequence: event.sequence,
                event_id: event.event_id.clone(),
                name: event.name,
                payload: event.payload,
            },
            Some(applied.id),
        )?;
        let cursor = self
            .external_cursors
            .entry(key)
            .or_insert_with(|| ExternalCursor {
                sequence: 0,
                event_id: String::new(),
                observed: BTreeMap::new(),
            });
        cursor.sequence = event.sequence;
        cursor.event_id = event.event_id.clone();
        cursor.observed.insert(event.sequence, event.event_id);
        self.pending.push_back((0, observed));
        self.drain_behaviors()?;
        Ok(ExternalProjectionOutcome::Applied)
    }

    fn drain_behaviors(&mut self) -> Result<(), RuntimeError> {
        while let Some((depth, triggering_event)) = self.pending.pop_front() {
            if depth >= self.limits.max_behavior_depth {
                return Err(RuntimeError::BehaviorDepthExceeded(
                    self.limits.max_behavior_depth,
                ));
            }
            let matching = self
                .behaviors
                .iter()
                .filter(|behavior| behavior.filter().matches(&triggering_event, &self.graph))
                .cloned()
                .collect::<Vec<_>>();
            for behavior in matching {
                let cause = Some(triggering_event.id.clone());
                self.append(
                    GraphEvent::BehaviorStarted {
                        name: behavior.name().to_string(),
                    },
                    cause.clone(),
                )?;
                let result = behavior.evaluate(BehaviorContext {
                    graph: &self.graph,
                    event: &triggering_event,
                });
                match result {
                    Ok(patches) => {
                        for patch in patches {
                            self.apply_patch(patch, cause.clone(), depth + 1)?;
                        }
                        self.append(
                            GraphEvent::BehaviorCompleted {
                                name: behavior.name().to_string(),
                            },
                            cause,
                        )?;
                    }
                    Err(error) => {
                        self.append(
                            GraphEvent::BehaviorFailed {
                                name: behavior.name().to_string(),
                                error: error.to_string(),
                            },
                            cause,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_patch(
        &mut self,
        patch: GraphPatch,
        causation_id: Option<String>,
        depth: usize,
    ) -> Result<bool, RuntimeError> {
        let patch_id = new_id("patch");
        let validation = self.validate_patch(&patch);
        let required_events = validation
            .as_ref()
            .map_or(2, |mutation_events| mutation_events.len().saturating_add(2));
        if self.events.len().saturating_add(required_events) > self.limits.max_events {
            return Err(RuntimeError::EventLimitExceeded(self.limits.max_events));
        }
        let proposed = self.append(
            GraphEvent::PatchProposed {
                patch_id: patch_id.clone(),
                patch: patch.clone(),
            },
            causation_id,
        )?;
        self.pending.push_back((depth, proposed.clone()));

        let mutation_events = match validation {
            Ok(events) => events,
            Err(reason) => {
                let rejected = self.append(
                    GraphEvent::PatchRejected { patch_id, reason },
                    Some(proposed.id),
                )?;
                self.pending.push_back((depth, rejected));
                return Ok(false);
            }
        };

        let mut last_cause = proposed.id;
        for event in mutation_events {
            let record = self.append(event, Some(last_cause))?;
            last_cause = record.id.clone();
            self.pending.push_back((depth, record));
        }
        let applied = self.append(GraphEvent::PatchApplied { patch_id }, Some(last_cause))?;
        self.pending.push_back((depth, applied));
        Ok(true)
    }

    fn validate_patch(&self, patch: &GraphPatch) -> Result<Vec<GraphEvent>, String> {
        if patch.expected_graph_version != self.graph.version() {
            return Err(format!(
                "graph version conflict: expected {}, current {}",
                patch.expected_graph_version,
                self.graph.version()
            ));
        }
        let mut candidate = PatchValidationView::new(&self.graph);
        let mut events = Vec::with_capacity(patch.operations.len());
        for operation in &patch.operations {
            events.push(candidate.apply(operation)?);
        }
        Ok(events)
    }

    fn append(
        &mut self,
        event: GraphEvent,
        causation_id: Option<String>,
    ) -> Result<GraphEventRecord, RuntimeError> {
        if self.events.len() >= self.limits.max_events {
            return Err(RuntimeError::EventLimitExceeded(self.limits.max_events));
        }
        let state_version_before = self.graph.version();
        let mut structural_hash = self.structural_hash;
        structural_hash.apply(&event, &self.graph)?;
        self.graph.apply(&event)?;
        self.structural_hash = structural_hash;
        let state_hash_after = if self.graph.version() == state_version_before {
            match self.events.last() {
                Some(record) if record.schema_version == GRAPH_EVENT_SCHEMA_VERSION => {
                    record.state_hash_after.clone()
                }
                _ => self.structural_hash.digest(self.graph.version())?,
            }
        } else {
            self.structural_hash.digest(self.graph.version())?
        };
        let mut record = GraphEventRecord {
            schema_version: GRAPH_EVENT_SCHEMA_VERSION,
            id: new_id("event"),
            sequence: self.events.len() as u64,
            timestamp_ms: now_ms(),
            branch_id: self.branch_id.clone(),
            causation_id,
            correlation_id: self.correlation_id.clone(),
            state_version_before,
            state_version_after: self.graph.version(),
            state_hash_after,
            previous_record_hash: self.events.last().map(|record| record.record_hash.clone()),
            record_hash: String::new(),
            event,
        };
        record.record_hash = record_hash(&record)?;
        self.events.push(record.clone());
        Ok(record)
    }
}

impl Default for GraphRuntime {
    fn default() -> Self {
        Self::new()
    }
}

struct PatchValidationView<'a> {
    graph: &'a StateGraph,
    objects: BTreeMap<String, Option<GraphObject>>,
    relations: BTreeMap<String, Option<GraphRelation>>,
}

impl<'a> PatchValidationView<'a> {
    fn new(graph: &'a StateGraph) -> Self {
        Self {
            graph,
            objects: BTreeMap::new(),
            relations: BTreeMap::new(),
        }
    }

    fn object(&self, id: &str) -> Option<&GraphObject> {
        self.objects
            .get(id)
            .map_or_else(|| self.graph.object(id), Option::as_ref)
    }

    fn relation(&self, id: &str) -> Option<&GraphRelation> {
        self.relations
            .get(id)
            .map_or_else(|| self.graph.relation(id), Option::as_ref)
    }

    fn object_has_relations(&self, id: &str) -> bool {
        self.graph.relations().any(|base| {
            self.relation(&base.id)
                .is_some_and(|relation| relation.source == id || relation.target == id)
        }) || self.relations.values().flatten().any(|relation| {
            self.graph.relation(&relation.id).is_none()
                && (relation.source == id || relation.target == id)
        })
    }

    fn apply(&mut self, operation: &PatchOperation) -> Result<GraphEvent, String> {
        Ok(match operation {
            PatchOperation::AddObject {
                id,
                object_type,
                data,
            } => {
                if self.object(id).is_some() {
                    return Err(format!("object `{id}` already exists"));
                }
                self.objects.insert(
                    id.clone(),
                    Some(GraphObject {
                        id: id.clone(),
                        object_type: object_type.clone(),
                        data: data.clone(),
                        version: 1,
                    }),
                );
                GraphEvent::ObjectCreated {
                    id: id.clone(),
                    object_type: object_type.clone(),
                    data: data.clone(),
                }
            }
            PatchOperation::UpdateObject {
                id,
                expected_version,
                data,
            } => {
                let current = self
                    .object(id)
                    .cloned()
                    .ok_or_else(|| format!("object `{id}` does not exist"))?;
                if current.version != *expected_version {
                    return Err(format!(
                        "object `{id}` version conflict: expected {expected_version}, current {}",
                        current.version
                    ));
                }
                let version = current.version + 1;
                self.objects.insert(
                    id.clone(),
                    Some(GraphObject {
                        data: data.clone(),
                        version,
                        ..current
                    }),
                );
                GraphEvent::ObjectUpdated {
                    id: id.clone(),
                    version,
                    data: data.clone(),
                }
            }
            PatchOperation::RemoveObject {
                id,
                expected_version,
            } => {
                let current = self
                    .object(id)
                    .cloned()
                    .ok_or_else(|| format!("object `{id}` does not exist"))?;
                if current.version != *expected_version {
                    return Err(format!("object `{id}` version conflict"));
                }
                if self.object_has_relations(id) {
                    return Err(format!("object `{id}` still has relations"));
                }
                self.objects.insert(id.clone(), None);
                GraphEvent::ObjectRemoved {
                    id: id.clone(),
                    version: current.version + 1,
                }
            }
            PatchOperation::AddRelation {
                id,
                relation_type,
                source,
                target,
                data,
            } => {
                if self.relation(id).is_some() {
                    return Err(format!("relation `{id}` already exists"));
                }
                if self.object(source).is_none() || self.object(target).is_none() {
                    return Err(format!("relation `{id}` has a missing endpoint"));
                }
                self.relations.insert(
                    id.clone(),
                    Some(GraphRelation {
                        id: id.clone(),
                        relation_type: relation_type.clone(),
                        source: source.clone(),
                        target: target.clone(),
                        data: data.clone(),
                        version: 1,
                    }),
                );
                GraphEvent::RelationCreated {
                    id: id.clone(),
                    relation_type: relation_type.clone(),
                    source: source.clone(),
                    target: target.clone(),
                    data: data.clone(),
                }
            }
            PatchOperation::UpdateRelation {
                id,
                expected_version,
                data,
            } => {
                let current = self
                    .relation(id)
                    .cloned()
                    .ok_or_else(|| format!("relation `{id}` does not exist"))?;
                if current.version != *expected_version {
                    return Err(format!("relation `{id}` version conflict"));
                }
                let version = current.version + 1;
                self.relations.insert(
                    id.clone(),
                    Some(GraphRelation {
                        data: data.clone(),
                        version,
                        ..current
                    }),
                );
                GraphEvent::RelationUpdated {
                    id: id.clone(),
                    version,
                    data: data.clone(),
                }
            }
            PatchOperation::RemoveRelation {
                id,
                expected_version,
            } => {
                let current = self
                    .relation(id)
                    .cloned()
                    .ok_or_else(|| format!("relation `{id}` does not exist"))?;
                if current.version != *expected_version {
                    return Err(format!("relation `{id}` version conflict"));
                }
                self.relations.insert(id.clone(), None);
                GraphEvent::RelationRemoved {
                    id: id.clone(),
                    version: current.version + 1,
                }
            }
        })
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn external_cursors(
    events: &[GraphEventRecord],
) -> Result<BTreeMap<(String, String), ExternalCursor>, ReplayError> {
    let mut cursors: BTreeMap<(String, String), ExternalCursor> = BTreeMap::new();
    for record in events {
        let GraphEvent::ExternalEventObserved {
            source,
            stream_id,
            sequence,
            event_id,
            ..
        } = &record.event
        else {
            continue;
        };
        let key = (source.clone(), stream_id.clone());
        let expected = cursors
            .get(&key)
            .map_or(1, |cursor| cursor.sequence.saturating_add(1));
        if *sequence != expected {
            return Err(ReplayError::InvalidMutation(format!(
                "external stream `{source}/{stream_id}` sequence diverged: expected {expected}, got {sequence}"
            )));
        }
        let cursor = cursors.entry(key).or_insert_with(|| ExternalCursor {
            sequence: 0,
            event_id: String::new(),
            observed: BTreeMap::new(),
        });
        cursor.sequence = *sequence;
        cursor.event_id = event_id.clone();
        cursor.observed.insert(*sequence, event_id.clone());
    }
    Ok(cursors)
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error("behavior `{0}` is already registered")]
    DuplicateBehavior(String),
    #[error("event limit of {0} exceeded")]
    EventLimitExceeded(usize),
    #[error("behavior recursion depth of {0} exceeded")]
    BehaviorDepthExceeded(usize),
    #[error("cannot fork at exclusive sequence {0}")]
    InvalidFork(u64),
    #[error("external stream `{source_name}/{stream_id}` sequence diverged: expected {expected}, got {actual}")]
    ExternalSequenceDiverged {
        source_name: String,
        stream_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("external stream `{source_name}/{stream_id}` event at sequence {sequence} conflicts with the observed event id")]
    ExternalEventConflict {
        source_name: String,
        stream_id: String,
        sequence: u64,
    },
    #[error("invalid external projection: {0}")]
    InvalidExternalProjection(String),
}
