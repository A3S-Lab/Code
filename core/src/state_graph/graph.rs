use super::event::MIN_GRAPH_EVENT_SCHEMA_VERSION;
use super::{GraphEvent, GraphEventRecord, GraphObject, GraphRelation, GRAPH_EVENT_SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateGraph {
    version: u64,
    objects: BTreeMap<String, GraphObject>,
    relations: BTreeMap<String, GraphRelation>,
}

impl StateGraph {
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn objects(&self) -> impl Iterator<Item = &GraphObject> {
        self.objects.values()
    }

    pub fn relations(&self) -> impl Iterator<Item = &GraphRelation> {
        self.relations.values()
    }

    pub fn object(&self, id: &str) -> Option<&GraphObject> {
        self.objects.get(id)
    }

    pub fn relation(&self, id: &str) -> Option<&GraphRelation> {
        self.relations.get(id)
    }

    pub fn relations_from<'a>(
        &'a self,
        source: &'a str,
    ) -> impl Iterator<Item = &'a GraphRelation> + 'a {
        self.relations.values().filter(move |r| r.source == source)
    }

    pub fn relations_to<'a>(
        &'a self,
        target: &'a str,
    ) -> impl Iterator<Item = &'a GraphRelation> + 'a {
        self.relations.values().filter(move |r| r.target == target)
    }

    pub(crate) fn apply(&mut self, event: &GraphEvent) -> Result<bool, ReplayError> {
        let mutated = match event {
            GraphEvent::ObjectCreated {
                id,
                object_type,
                data,
            } => {
                if self.objects.contains_key(id) {
                    return Err(ReplayError::InvalidMutation(format!(
                        "object `{id}` already exists"
                    )));
                }
                self.objects.insert(
                    id.clone(),
                    GraphObject {
                        id: id.clone(),
                        object_type: object_type.clone(),
                        data: data.clone(),
                        version: 1,
                    },
                );
                true
            }
            GraphEvent::ObjectUpdated { id, version, data } => {
                let object = self.objects.get_mut(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("object `{id}` does not exist"))
                })?;
                if *version != object.version + 1 {
                    return Err(ReplayError::InvalidMutation(format!(
                        "object `{id}` version jumped from {} to {version}",
                        object.version
                    )));
                }
                object.version = *version;
                object.data = data.clone();
                true
            }
            GraphEvent::ObjectRemoved { id, version } => {
                let object = self.objects.get(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("object `{id}` does not exist"))
                })?;
                if *version != object.version + 1 {
                    return Err(ReplayError::InvalidMutation(format!(
                        "object `{id}` removal version is stale"
                    )));
                }
                if self
                    .relations
                    .values()
                    .any(|relation| relation.source == *id || relation.target == *id)
                {
                    return Err(ReplayError::InvalidMutation(format!(
                        "object `{id}` still has relations"
                    )));
                }
                self.objects.remove(id);
                true
            }
            GraphEvent::RelationCreated {
                id,
                relation_type,
                source,
                target,
                data,
            } => {
                if self.relations.contains_key(id) {
                    return Err(ReplayError::InvalidMutation(format!(
                        "relation `{id}` already exists"
                    )));
                }
                if !self.objects.contains_key(source) || !self.objects.contains_key(target) {
                    return Err(ReplayError::InvalidMutation(format!(
                        "relation `{id}` has a missing endpoint"
                    )));
                }
                self.relations.insert(
                    id.clone(),
                    GraphRelation {
                        id: id.clone(),
                        relation_type: relation_type.clone(),
                        source: source.clone(),
                        target: target.clone(),
                        data: data.clone(),
                        version: 1,
                    },
                );
                true
            }
            GraphEvent::RelationUpdated { id, version, data } => {
                let relation = self.relations.get_mut(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("relation `{id}` does not exist"))
                })?;
                if *version != relation.version + 1 {
                    return Err(ReplayError::InvalidMutation(format!(
                        "relation `{id}` version is stale"
                    )));
                }
                relation.version = *version;
                relation.data = data.clone();
                true
            }
            GraphEvent::RelationRemoved { id, version } => {
                let relation = self.relations.get(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("relation `{id}` does not exist"))
                })?;
                if *version != relation.version + 1 {
                    return Err(ReplayError::InvalidMutation(format!(
                        "relation `{id}` removal version is stale"
                    )));
                }
                self.relations.remove(id);
                true
            }
            _ => false,
        };
        if mutated {
            self.version += 1;
        }
        Ok(mutated)
    }

    pub fn diff(&self, other: &Self) -> GraphDiff {
        GraphDiff::between(self, other)
    }

    pub fn state_hash(&self) -> Result<String, ReplayError> {
        StructuralHash::from_graph(self)?.digest(self.version)
    }

    fn legacy_state_hash(&self) -> Result<String, ReplayError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ReplayError::Serialization(error.to_string()))?;
        Ok(sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StructuralHash([u8; 32]);

impl StructuralHash {
    pub(crate) fn from_graph(graph: &StateGraph) -> Result<Self, ReplayError> {
        let mut hash = Self::default();
        for object in graph.objects.values() {
            hash.add(contribution(b"object", object)?);
        }
        for relation in graph.relations.values() {
            hash.add(contribution(b"relation", relation)?);
        }
        Ok(hash)
    }

    pub(crate) fn apply(
        &mut self,
        event: &GraphEvent,
        graph: &StateGraph,
    ) -> Result<(), ReplayError> {
        match event {
            GraphEvent::ObjectCreated {
                id,
                object_type,
                data,
            } => self.add(contribution(
                b"object",
                &GraphObject {
                    id: id.clone(),
                    object_type: object_type.clone(),
                    data: data.clone(),
                    version: 1,
                },
            )?),
            GraphEvent::ObjectUpdated { id, version, data } => {
                let current = graph.object(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("object `{id}` does not exist"))
                })?;
                self.subtract(contribution(b"object", current)?);
                self.add(contribution(
                    b"object",
                    &GraphObject {
                        id: id.clone(),
                        object_type: current.object_type.clone(),
                        data: data.clone(),
                        version: *version,
                    },
                )?);
            }
            GraphEvent::ObjectRemoved { id, .. } => {
                let current = graph.object(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("object `{id}` does not exist"))
                })?;
                self.subtract(contribution(b"object", current)?);
            }
            GraphEvent::RelationCreated {
                id,
                relation_type,
                source,
                target,
                data,
            } => self.add(contribution(
                b"relation",
                &GraphRelation {
                    id: id.clone(),
                    relation_type: relation_type.clone(),
                    source: source.clone(),
                    target: target.clone(),
                    data: data.clone(),
                    version: 1,
                },
            )?),
            GraphEvent::RelationUpdated { id, version, data } => {
                let current = graph.relation(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("relation `{id}` does not exist"))
                })?;
                self.subtract(contribution(b"relation", current)?);
                self.add(contribution(
                    b"relation",
                    &GraphRelation {
                        id: id.clone(),
                        relation_type: current.relation_type.clone(),
                        source: current.source.clone(),
                        target: current.target.clone(),
                        data: data.clone(),
                        version: *version,
                    },
                )?);
            }
            GraphEvent::RelationRemoved { id, .. } => {
                let current = graph.relation(id).ok_or_else(|| {
                    ReplayError::InvalidMutation(format!("relation `{id}` does not exist"))
                })?;
                self.subtract(contribution(b"relation", current)?);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn digest(&self, version: u64) -> Result<String, ReplayError> {
        let mut bytes = b"a3s-state-graph-v2\0".to_vec();
        bytes.extend_from_slice(&version.to_be_bytes());
        bytes.extend_from_slice(&self.0);
        Ok(sha256::digest(bytes))
    }

    fn add(&mut self, value: [u8; 32]) {
        let mut carry = 0u16;
        for index in (0..32).rev() {
            let sum = u16::from(self.0[index]) + u16::from(value[index]) + carry;
            self.0[index] = sum as u8;
            carry = sum >> 8;
        }
    }

    fn subtract(&mut self, value: [u8; 32]) {
        let mut borrow = 0i16;
        for index in (0..32).rev() {
            let difference = i16::from(self.0[index]) - i16::from(value[index]) - borrow;
            if difference < 0 {
                self.0[index] = (difference + 256) as u8;
                borrow = 1;
            } else {
                self.0[index] = difference as u8;
                borrow = 0;
            }
        }
    }
}

fn contribution<T: Serialize>(domain: &[u8], value: &T) -> Result<[u8; 32], ReplayError> {
    let mut bytes = domain.to_vec();
    bytes.push(0);
    bytes.extend(
        serde_json::to_vec(value).map_err(|error| ReplayError::Serialization(error.to_string()))?,
    );
    let encoded = sha256::digest(bytes);
    let mut output = [0u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16)
            .map_err(|error| ReplayError::Serialization(error.to_string()))?;
    }
    Ok(output)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphDiff {
    pub objects_added: Vec<GraphObject>,
    pub objects_removed: Vec<GraphObject>,
    pub objects_changed: Vec<(GraphObject, GraphObject)>,
    pub relations_added: Vec<GraphRelation>,
    pub relations_removed: Vec<GraphRelation>,
    pub relations_changed: Vec<(GraphRelation, GraphRelation)>,
}

impl GraphDiff {
    fn between(left: &StateGraph, right: &StateGraph) -> Self {
        let object_ids = left
            .objects
            .keys()
            .chain(right.objects.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let relation_ids = left
            .relations
            .keys()
            .chain(right.relations.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut diff = Self::default();
        for id in object_ids {
            match (left.objects.get(&id), right.objects.get(&id)) {
                (None, Some(value)) => diff.objects_added.push(value.clone()),
                (Some(value), None) => diff.objects_removed.push(value.clone()),
                (Some(a), Some(b)) if a != b => diff.objects_changed.push((a.clone(), b.clone())),
                _ => {}
            }
        }
        for id in relation_ids {
            match (left.relations.get(&id), right.relations.get(&id)) {
                (None, Some(value)) => diff.relations_added.push(value.clone()),
                (Some(value), None) => diff.relations_removed.push(value.clone()),
                (Some(a), Some(b)) if a != b => diff.relations_changed.push((a.clone(), b.clone())),
                _ => {}
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.objects_added.is_empty()
            && self.objects_removed.is_empty()
            && self.objects_changed.is_empty()
            && self.relations_added.is_empty()
            && self.relations_removed.is_empty()
            && self.relations_changed.is_empty()
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("unsupported graph event schema version {actual}; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error(
        "graph event schema downgraded at sequence {sequence}: previous {previous}, got {actual}"
    )]
    SchemaDowngrade {
        sequence: u64,
        previous: u32,
        actual: u32,
    },
    #[error("event sequence diverged: expected {expected}, got {actual}")]
    SequenceDiverged { expected: u64, actual: u64 },
    #[error("state version diverged at sequence {sequence}: expected {expected}, got {actual}")]
    StateVersionDiverged {
        sequence: u64,
        expected: u64,
        actual: u64,
    },
    #[error("event `{event_id}` references unknown or future cause `{causation_id}`")]
    InvalidCausation {
        event_id: String,
        causation_id: String,
    },
    #[error("duplicate event id `{0}`")]
    DuplicateEventId(String),
    #[error("state hash diverged at sequence {sequence}")]
    StateHashDiverged { sequence: u64 },
    #[error("record hash chain diverged at sequence {sequence}")]
    RecordHashDiverged { sequence: u64 },
    #[error("invalid graph mutation: {0}")]
    InvalidMutation(String),
    #[error("failed to serialize graph replay state: {0}")]
    Serialization(String),
}

pub(crate) fn replay_strict(records: &[GraphEventRecord]) -> Result<StateGraph, ReplayError> {
    let mut graph = StateGraph::default();
    let mut seen_ids = BTreeSet::new();
    let mut structural_hash = StructuralHash::default();
    let mut legacy_state_hash = graph.legacy_state_hash()?;
    let mut previous_schema = MIN_GRAPH_EVENT_SCHEMA_VERSION;
    for (expected_sequence, record) in records.iter().enumerate() {
        if !(MIN_GRAPH_EVENT_SCHEMA_VERSION..=GRAPH_EVENT_SCHEMA_VERSION)
            .contains(&record.schema_version)
        {
            return Err(ReplayError::UnsupportedSchema {
                expected: GRAPH_EVENT_SCHEMA_VERSION,
                actual: record.schema_version,
            });
        }
        let expected_sequence = expected_sequence as u64;
        if record.schema_version < previous_schema {
            return Err(ReplayError::SchemaDowngrade {
                sequence: expected_sequence,
                previous: previous_schema,
                actual: record.schema_version,
            });
        }
        if record.sequence != expected_sequence {
            return Err(ReplayError::SequenceDiverged {
                expected: expected_sequence,
                actual: record.sequence,
            });
        }
        if let Some(cause) = &record.causation_id {
            if !seen_ids.contains(cause) {
                return Err(ReplayError::InvalidCausation {
                    event_id: record.id.clone(),
                    causation_id: cause.clone(),
                });
            }
        }
        if seen_ids.contains(&record.id) {
            return Err(ReplayError::DuplicateEventId(record.id.clone()));
        }
        let expected_previous = expected_sequence
            .checked_sub(1)
            .map(|index| records[index as usize].record_hash.clone());
        if record.previous_record_hash != expected_previous
            || record.record_hash != record_hash(record)?
        {
            return Err(ReplayError::RecordHashDiverged {
                sequence: record.sequence,
            });
        }
        if record.state_version_before != graph.version {
            return Err(ReplayError::StateVersionDiverged {
                sequence: record.sequence,
                expected: graph.version,
                actual: record.state_version_before,
            });
        }
        let mut next_structural_hash = structural_hash;
        next_structural_hash.apply(&record.event, &graph)?;
        let mutated = graph.apply(&record.event)?;
        structural_hash = next_structural_hash;
        if record.state_version_after != graph.version {
            return Err(ReplayError::StateVersionDiverged {
                sequence: record.sequence,
                expected: graph.version,
                actual: record.state_version_after,
            });
        }
        if mutated && record.schema_version == 1 {
            legacy_state_hash = graph.legacy_state_hash()?;
        }
        let expected_state_hash = if record.schema_version == 1 {
            legacy_state_hash.clone()
        } else {
            legacy_state_hash = String::new();
            structural_hash.digest(graph.version)?
        };
        if record.state_hash_after != expected_state_hash {
            return Err(ReplayError::StateHashDiverged {
                sequence: record.sequence,
            });
        }
        seen_ids.insert(record.id.clone());
        previous_schema = record.schema_version;
    }
    Ok(graph)
}

pub(crate) fn record_hash(record: &GraphEventRecord) -> Result<String, ReplayError> {
    let value = serde_json::json!({
        "schema_version": record.schema_version,
        "id": record.id,
        "sequence": record.sequence,
        "timestamp_ms": record.timestamp_ms,
        "branch_id": record.branch_id,
        "causation_id": record.causation_id,
        "correlation_id": record.correlation_id,
        "state_version_before": record.state_version_before,
        "state_version_after": record.state_version_after,
        "state_hash_after": record.state_hash_after,
        "previous_record_hash": record.previous_record_hash,
        "event": record.event,
    });
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| ReplayError::Serialization(error.to_string()))?;
    Ok(sha256::digest(bytes))
}
