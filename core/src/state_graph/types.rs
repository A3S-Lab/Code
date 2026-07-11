use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ObjectId = String;
pub type RelationId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphObject {
    pub id: ObjectId,
    #[serde(rename = "type")]
    pub object_type: String,
    pub data: Value,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphRelation {
    pub id: RelationId,
    #[serde(rename = "type")]
    pub relation_type: String,
    pub source: ObjectId,
    pub target: ObjectId,
    #[serde(default)]
    pub data: Value,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphPatch {
    pub expected_graph_version: u64,
    pub operations: Vec<PatchOperation>,
}

impl GraphPatch {
    pub fn new(expected_graph_version: u64, operations: Vec<PatchOperation>) -> Self {
        Self {
            expected_graph_version,
            operations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOperation {
    AddObject {
        id: ObjectId,
        object_type: String,
        data: Value,
    },
    UpdateObject {
        id: ObjectId,
        expected_version: u64,
        data: Value,
    },
    RemoveObject {
        id: ObjectId,
        expected_version: u64,
    },
    AddRelation {
        id: RelationId,
        relation_type: String,
        source: ObjectId,
        target: ObjectId,
        #[serde(default)]
        data: Value,
    },
    UpdateRelation {
        id: RelationId,
        expected_version: u64,
        data: Value,
    },
    RemoveRelation {
        id: RelationId,
        expected_version: u64,
    },
}
