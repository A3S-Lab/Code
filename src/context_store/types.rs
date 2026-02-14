//! Core types for context store

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::digest::Digest;
use super::pathway::Pathway;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Document,
    Code,
    Markdown,
    Memory,
    Capability,
    Message,
    Data,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub pathway: Pathway,
    pub kind: NodeKind,
    pub content: String,
    pub digest: Digest,
    pub embedding: Vec<f32>,
    pub metadata: std::collections::HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub children: Vec<Pathway>,
}

impl Node {
    pub fn new(pathway: Pathway, kind: NodeKind, content: String) -> Self {
        let now = Utc::now();
        Self {
            pathway,
            kind,
            content,
            digest: Digest::new(),
            embedding: Vec::new(),
            metadata: std::collections::HashMap::new(),
            created_at: now,
            updated_at: now,
            children: Vec::new(),
        }
    }

    pub fn update_content(&mut self, content: String) {
        self.content = content;
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_kind_serialization() {
        let kind = NodeKind::Code;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"code\"");
        let parsed: NodeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, NodeKind::Code);
    }

    #[test]
    fn test_node_new() {
        let pathway = Pathway::parse("a3s://test/doc").unwrap();
        let node = Node::new(pathway.clone(), NodeKind::Document, "hello".to_string());
        assert_eq!(node.kind, NodeKind::Document);
        assert_eq!(node.content, "hello");
        assert!(node.embedding.is_empty());
        assert!(!node.digest.is_generated());
    }

    #[test]
    fn test_node_update_content() {
        let pathway = Pathway::parse("a3s://test/doc").unwrap();
        let mut node = Node::new(pathway, NodeKind::Document, "old".to_string());
        let old_updated = node.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        node.update_content("new".to_string());
        assert_eq!(node.content, "new");
        assert!(node.updated_at >= old_updated);
    }

    #[test]
    fn test_all_node_kinds() {
        let kinds = vec![
            NodeKind::Document, NodeKind::Code, NodeKind::Markdown,
            NodeKind::Memory, NodeKind::Capability, NodeKind::Message,
            NodeKind::Data, NodeKind::Directory,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let parsed: NodeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }
}
