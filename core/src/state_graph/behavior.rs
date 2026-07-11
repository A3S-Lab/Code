use super::{GraphEvent, GraphEventRecord, GraphPatch, StateGraph};
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

type EventPredicate = dyn Fn(&GraphEventRecord, &StateGraph) -> bool + Send + Sync;

#[derive(Clone, Default)]
pub struct EventFilter {
    event_types: Vec<String>,
    object_types: Vec<String>,
    relation_types: Vec<String>,
    predicate: Option<Arc<EventPredicate>>,
}

impl EventFilter {
    pub fn new(event_types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            event_types: event_types.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn with_object_types(
        mut self,
        object_types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.object_types = object_types.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_relation_types(
        mut self,
        relation_types: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.relation_types = relation_types.into_iter().map(Into::into).collect();
        self
    }

    pub fn where_predicate(
        mut self,
        predicate: impl Fn(&GraphEventRecord, &StateGraph) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.predicate = Some(Arc::new(predicate));
        self
    }

    pub fn matches(&self, event: &GraphEventRecord, graph: &StateGraph) -> bool {
        let event_type_matches = self.event_types.is_empty()
            || self
                .event_types
                .iter()
                .any(|candidate| candidate == event.event.event_type());
        let object_type_matches = self.object_types.is_empty()
            || event_object_type(event, graph).is_some_and(|object_type| {
                self.object_types
                    .iter()
                    .any(|candidate| candidate == object_type)
            });
        let relation_type_matches = self.relation_types.is_empty()
            || event_relation_type(event, graph).is_some_and(|relation_type| {
                self.relation_types
                    .iter()
                    .any(|candidate| candidate == relation_type)
            });
        event_type_matches
            && object_type_matches
            && relation_type_matches
            && self
                .predicate
                .as_ref()
                .is_none_or(|predicate| predicate(event, graph))
    }
}

impl fmt::Debug for EventFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventFilter")
            .field("event_types", &self.event_types)
            .field("object_types", &self.object_types)
            .field("relation_types", &self.relation_types)
            .field("has_predicate", &self.predicate.is_some())
            .finish()
    }
}

fn event_object_type<'a>(event: &'a GraphEventRecord, graph: &'a StateGraph) -> Option<&'a str> {
    match &event.event {
        GraphEvent::ObjectCreated { object_type, .. } => Some(object_type),
        GraphEvent::ObjectUpdated { id, .. } => {
            graph.object(id).map(|object| object.object_type.as_str())
        }
        GraphEvent::ObjectRemoved { .. } => None,
        _ => None,
    }
}

fn event_relation_type<'a>(event: &'a GraphEventRecord, graph: &'a StateGraph) -> Option<&'a str> {
    match &event.event {
        GraphEvent::RelationCreated { relation_type, .. } => Some(relation_type),
        GraphEvent::RelationUpdated { id, .. } => graph
            .relation(id)
            .map(|relation| relation.relation_type.as_str()),
        GraphEvent::RelationRemoved { .. } => None,
        _ => None,
    }
}

pub struct BehaviorContext<'a> {
    pub graph: &'a StateGraph,
    pub event: &'a GraphEventRecord,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct BehaviorError {
    pub message: String,
}

impl BehaviorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait Behavior: Send + Sync {
    fn name(&self) -> &str;
    fn filter(&self) -> &EventFilter;
    fn evaluate(&self, context: BehaviorContext<'_>) -> Result<Vec<GraphPatch>, BehaviorError>;
}

type BehaviorFn =
    dyn for<'a> Fn(BehaviorContext<'a>) -> Result<Vec<GraphPatch>, BehaviorError> + Send + Sync;

pub struct FnBehavior {
    name: String,
    filter: EventFilter,
    evaluator: Arc<BehaviorFn>,
}

impl FnBehavior {
    pub fn new(
        name: impl Into<String>,
        filter: EventFilter,
        evaluator: impl for<'a> Fn(BehaviorContext<'a>) -> Result<Vec<GraphPatch>, BehaviorError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            filter,
            evaluator: Arc::new(evaluator),
        }
    }
}

impl Behavior for FnBehavior {
    fn name(&self) -> &str {
        &self.name
    }

    fn filter(&self) -> &EventFilter {
        &self.filter
    }

    fn evaluate(&self, context: BehaviorContext<'_>) -> Result<Vec<GraphPatch>, BehaviorError> {
        (self.evaluator)(context)
    }
}
