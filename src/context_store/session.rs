//! Session management for conversation tracking

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use super::config::Config;
use super::embedding::Embedder;
use super::error::Result;
use super::pathway::Pathway;
use super::storage::StorageBackend;

#[derive(Clone)]
pub struct Session {
    id: String,
    #[allow(dead_code)]
    user: String,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
    messages: Vec<Message>,
    #[allow(dead_code)]
    storage: Arc<dyn StorageBackend>,
    #[allow(dead_code)]
    embedder: Arc<dyn Embedder>,
    #[allow(dead_code)]
    config: Config,
}

impl Session {
    pub async fn new(id: Option<&str>, storage: Arc<dyn StorageBackend>, embedder: Arc<dyn Embedder>, config: &Config) -> Result<Self> {
        let id = id.map(|s| s.to_string()).unwrap_or_else(|| Uuid::new_v4().to_string());
        Ok(Self { id, user: "default".to_string(), created_at: Utc::now(), messages: Vec::new(), storage, embedder, config: config.clone() })
    }

    pub fn id(&self) -> &str { &self.id }

    pub fn add_message(&mut self, role: MessageRole, content: String) {
        self.messages.push(Message { role, content, timestamp: Utc::now(), contexts_used: Vec::new() });
    }

    pub fn messages(&self) -> &[Message] { &self.messages }

    pub async fn commit(&mut self) -> Result<()> {
        let _pathway = Pathway::parse(&format!("a3s://session/{}", self.id))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub contexts_used: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole { User, Assistant, System }

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::{Config, VectorIndexConfig};
    use super::super::embedding::MockEmbedder;
    use super::super::storage::MemoryStorage;

    fn create_test_embedder() -> Arc<dyn Embedder> { Arc::new(MockEmbedder::new(128)) }
    fn create_test_storage() -> Arc<dyn StorageBackend> {
        let config = VectorIndexConfig { index_type: "hnsw".to_string(), hnsw_m: 16, hnsw_ef_construction: 200 };
        Arc::new(MemoryStorage::new(&config))
    }

    #[tokio::test]
    async fn test_session_new_with_id() {
        let session = Session::new(Some("test-id"), create_test_storage(), create_test_embedder(), &Config::default()).await.unwrap();
        assert_eq!(session.id(), "test-id");
    }

    #[tokio::test]
    async fn test_session_add_message() {
        let mut session = Session::new(None, create_test_storage(), create_test_embedder(), &Config::default()).await.unwrap();
        session.add_message(MessageRole::User, "Hello".to_string());
        assert_eq!(session.messages().len(), 1);
    }

    #[test]
    fn test_message_role_serialization() {
        let json = serde_json::to_string(&MessageRole::User).unwrap();
        assert_eq!(json, "\"user\"");
    }
}
