//! a3s-vec backed durable memory store — the convergence target for A3S
//! memory.
//!
//! One system, A3S's own engine: memory items live in an in-process a3s-vec
//! collection with a full-text index over their content, so recall is a real
//! relevance query instead of loading every item. Importance and type ride
//! alongside as typed fields; vector (semantic) retrieval slots into the same
//! collection once an embedding source is wired.
//!
//! This module intentionally depends only on `a3s-vec` (already optional in
//! core behind `a3s-vec-fts`) so the memory layer and the workspace lexical
//! index share one engine and one operational model.

use a3s_memory::{MemoryItem, MemoryStore, MemoryType};
use a3s_vec::{Collection, CollectionSchema, DataType, Doc, FieldSchema, IndexParams, SearchQuery};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ENGINE_INIT: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_initialized() -> Result<(), String> {
    // `a3s_vec::initialize` is idempotent; always call it so the cold path
    // stays reachable under the process-wide OnceLock.
    ENGINE_INIT
        .get_or_init(|| a3s_vec::initialize(None).map_err(|error| error.to_string()))
        .clone()
}

fn display_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(error.to_string())
}

/// One durable memory record as stored in the a3s-vec collection.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecord {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub importance: f32,
    pub memory_type: String,
    /// RFC3339 timestamp (lexicographic order == chronological for UTC).
    pub timestamp: String,
    /// Serialized auxiliary state (metadata map, access counters).
    pub meta: String,
}

/// An a3s-vec collection holding durable memory items, opened at a fixed path.
pub struct VecMemoryStore {
    collection: Collection,
    path: PathBuf,
}

const COLLECTION_NAME: &str = "durable_memory";
const CONTENT_FIELD: &str = "content";

fn memory_schema() -> Result<CollectionSchema> {
    let mut content =
        FieldSchema::new(CONTENT_FIELD, DataType::String, false, 0).map_err(display_error)?;
    let fts = IndexParams::fts(Some("whitespace"), None, None).map_err(display_error)?;
    content.set_index_params(&fts).map_err(display_error)?;
    CollectionSchema::builder(COLLECTION_NAME)
        // The engine primary key (set on every Doc) IS the memory id; no
        // separate id field is declared.
        .add_field(content)
        .add_field(FieldSchema::new("tags", DataType::String, true, 0).map_err(display_error)?)
        .add_field(FieldSchema::new("importance", DataType::Float, true, 0).map_err(display_error)?)
        .add_field(
            FieldSchema::new("memory_type", DataType::String, true, 0).map_err(display_error)?,
        )
        .add_field(FieldSchema::new("timestamp", DataType::String, true, 0).map_err(display_error)?)
        .add_field(FieldSchema::new("meta", DataType::String, true, 0).map_err(display_error)?)
        .build()
        .map_err(display_error)
}

fn record_to_doc(record: &MemoryRecord) -> Result<Doc> {
    let mut doc = Doc::new().map_err(display_error)?;
    doc.set_pk(&record.id);
    doc.add_string(CONTENT_FIELD, &record.content)
        .map_err(display_error)?;
    doc.add_string("tags", &record.tags.join(" "))
        .map_err(display_error)?;
    doc.add_f32("importance", record.importance)
        .map_err(display_error)?;
    doc.add_string("memory_type", &record.memory_type)
        .map_err(display_error)?;
    doc.add_string("timestamp", &record.timestamp)
        .map_err(display_error)?;
    doc.add_string("meta", &record.meta)
        .map_err(display_error)?;
    Ok(doc)
}

fn memory_id(document: &Doc) -> Result<String> {
    document
        .get_pk()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("a3s-vec memory result omitted its primary key"))
}

impl VecMemoryStore {
    /// Open the memory collection at `root`, creating it on first use.
    pub fn open(root: &Path) -> Result<Self> {
        ensure_initialized().map_err(|error| anyhow!("a3s-vec engine init failed: {error}"))?;
        fs_err_create_dir_all(root)?;
        let collection_path = root.join(COLLECTION_NAME);
        let collection_path = collection_path
            .to_str()
            .ok_or_else(|| anyhow!("memory collection path is not UTF-8"))?;
        let collection = if collection_path_exists(&collection_path) {
            Collection::open(collection_path, None).map_err(display_error)?
        } else {
            let schema = memory_schema()?;
            Collection::create_and_open(collection_path, &schema, None).map_err(display_error)?
        };
        Ok(Self {
            collection,
            path: collection_path.into(),
        })
    }

    /// Insert or replace one memory record by id.
    pub fn put(&self, record: &MemoryRecord) -> Result<()> {
        let doc = record_to_doc(record)?;
        let references = [&doc];
        let result = self.collection.insert(&references).map_err(display_error)?;
        if result.error_count != 0 {
            return Err(anyhow!(
                "a3s-vec memory insert rejected {} document(s)",
                result.error_count
            ));
        }
        self.collection.flush().map_err(display_error)?;
        Ok(())
    }

    /// Full-text recall: rank memory records by FTS relevance to `query`.
    /// Returns `(id, score)` pairs, best first.
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut fts = a3s_vec::Fts::new().map_err(display_error)?;
        fts.set_match_string(query).map_err(display_error)?;
        let topk = i32::try_from(limit).map_err(|_| anyhow!("memory recall limit exceeds i32"))?;
        let mut search = SearchQuery::fts(CONTENT_FIELD, &fts, topk).map_err(display_error)?;
        search.set_output_fields(&[]).map_err(display_error)?;
        let documents = self.collection.query(&search).map_err(display_error)?;
        Ok(documents
            .iter()
            .map(memory_id)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .zip(documents.iter().map(Doc::get_score))
            .collect())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn fs_err_create_dir_all(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)
        .map_err(|error| anyhow!("create memory root {}: {error}", root.display()))
}

#[cfg(windows)]
fn collection_path_exists(path: &str) -> bool {
    Path::new(path).exists()
}

#[cfg(not(windows))]
fn collection_path_exists(path: &str) -> bool {
    Path::new(path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, content: &str, importance: f32) -> MemoryRecord {
        MemoryRecord {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec!["testing".to_owned()],
            importance,
            memory_type: "semantic".to_owned(),
            timestamp: "2026-01-01T00:00:00+00:00".to_owned(),
            meta: "{}".to_owned(),
        }
    }

    #[test]
    fn insert_and_fts_recall_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        store
            .put(&record(
                "m1",
                "the workspace uses rust-analyzer for navigation",
                0.8,
            ))
            .unwrap();
        store
            .put(&record(
                "m2",
                "e2e fixtures live under tests/e2e/fixtures",
                0.6,
            ))
            .unwrap();

        let hits = store.recall("rust-analyzer navigation", 5).unwrap();
        assert!(!hits.is_empty(), "expected at least one hit");
        assert_eq!(hits[0].0, "m1");
    }

    #[test]
    fn reopen_sees_persisted_records() {
        let root = tempfile::tempdir().unwrap();
        {
            let store = VecMemoryStore::open(root.path()).unwrap();
            store
                .put(&record("m3", "memory survives process restart", 0.5))
                .unwrap();
        }
        let reopened = VecMemoryStore::open(root.path()).unwrap();
        let hits = reopened.recall("process restart", 5).unwrap();
        assert!(hits.iter().any(|(id, _)| id == "m3"));
    }

    #[tokio::test]
    async fn memory_store_trait_roundtrip() {
        use a3s_memory::MemoryStore as _;
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        let mut item = MemoryItem::new("the workspace prefers rust-analyzer navigation");
        item.importance = 0.9;
        item.tags = vec!["testing".to_owned()];
        item.memory_type = MemoryType::Procedural;
        store.store(item.clone()).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);
        let fetched = store
            .retrieve(&item.id)
            .await
            .unwrap()
            .expect("stored item");
        assert_eq!(fetched.content, item.content);
        assert_eq!(fetched.memory_type, item.memory_type);

        let searched = store.search("rust-analyzer navigation", 5).await.unwrap();
        assert_eq!(searched.len(), 1);
        let by_tag = store
            .search_by_tags(&["testing".to_owned()], 5)
            .await
            .unwrap();
        assert_eq!(by_tag.len(), 1);
        let recent = store.get_recent(5).await.unwrap();
        assert_eq!(recent.len(), 1);
        let important = store.get_important(0.8, 5).await.unwrap();
        assert_eq!(important.len(), 1);
        let not_important = store.get_important(0.95, 5).await.unwrap();
        assert!(not_important.is_empty());

        store.delete(&item.id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
        assert!(store.retrieve(&item.id).await.unwrap().is_none());
    }

    #[test]
    fn empty_query_returns_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        store.put(&record("m4", "something", 0.5)).unwrap();
        assert!(store.recall("   ", 5).unwrap().is_empty());
    }
}

/// Metadata that rides with a record but does not need to be queryable.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct RecordMeta {
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    access_count: u32,
    #[serde(default)]
    last_accessed: Option<String>,
}

fn memory_type_to_string(memory_type: &MemoryType) -> Result<String> {
    serde_json::to_string(memory_type).map_err(display_error)
}

fn memory_type_from_string(raw: &str) -> Result<MemoryType> {
    serde_json::from_str(raw).map_err(display_error)
}

fn doc_to_item(document: &Doc) -> Result<MemoryItem> {
    let id = memory_id(document)?;
    let content = document
        .get_string(CONTENT_FIELD)
        .map_err(display_error)?
        .unwrap_or_default();
    let tags = document
        .get_string("tags")
        .map_err(display_error)?
        .unwrap_or_default();
    let importance = document
        .get_f32("importance")
        .map_err(display_error)?
        .unwrap_or_default();
    let memory_type = document
        .get_string("memory_type")
        .map_err(display_error)?
        .unwrap_or_default();
    let timestamp = document
        .get_string("timestamp")
        .map_err(display_error)?
        .unwrap_or_default();
    let meta_raw = document
        .get_string("meta")
        .map_err(display_error)?
        .unwrap_or_default();
    let meta: RecordMeta = serde_json::from_str(&meta_raw).unwrap_or_default();
    let timestamp = DateTime::parse_from_rfc3339(&timestamp)
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let memory_type = memory_type_from_string(&memory_type).unwrap_or(MemoryType::Semantic);
    Ok(MemoryItem {
        id,
        content,
        timestamp,
        importance,
        tags: tags.split_whitespace().map(str::to_owned).collect(),
        memory_type,
        metadata: meta.metadata,
        access_count: meta.access_count,
        last_accessed: meta.last_accessed.and_then(|raw| {
            DateTime::parse_from_rfc3339(&raw)
                .map(|parsed| parsed.with_timezone(&Utc))
                .ok()
        }),
        content_lower: String::new(),
    })
}

fn item_to_record(item: &MemoryItem) -> Result<MemoryRecord> {
    let meta = RecordMeta {
        metadata: item.metadata.clone(),
        access_count: item.access_count,
        last_accessed: item.last_accessed.map(|stamp| stamp.to_rfc3339()),
    };
    Ok(MemoryRecord {
        id: item.id.clone(),
        content: item.content.clone(),
        tags: item.tags.clone(),
        importance: item.importance,
        memory_type: memory_type_to_string(&item.memory_type)?,
        timestamp: item.timestamp.to_rfc3339(),
        meta: serde_json::to_string(&meta).map_err(display_error)?,
    })
}

#[async_trait::async_trait]
impl MemoryStore for VecMemoryStore {
    async fn store(&self, item: MemoryItem) -> Result<()> {
        self.put(&item_to_record(&item)?)
    }

    async fn retrieve(&self, id: &str) -> Result<Option<MemoryItem>> {
        for document in self.collection.iter().map_err(display_error)? {
            let document = document.map_err(display_error)?;
            if memory_id(&document)?.as_str() == id {
                return Ok(Some(doc_to_item(&document)?));
            }
        }
        Ok(None)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryItem>> {
        let hits = self.recall(query, limit)?;
        let wanted: std::collections::HashSet<&str> =
            hits.iter().map(|(id, _)| id.as_str()).collect();
        let mut items = Vec::with_capacity(hits.len());
        for document in self.collection.iter().map_err(display_error)? {
            let document = document.map_err(display_error)?;
            if memory_id(&document)?.as_str().is_empty()
                || !wanted.contains(memory_id(&document)?.as_str())
            {
                continue;
            }
            items.push(doc_to_item(&document)?);
            if items.len() == hits.len() {
                break;
            }
        }
        // Preserve FTS ranking order.
        items.sort_by_cached_key(|item| {
            hits.iter()
                .position(|(id, _)| id == &item.id)
                .unwrap_or(usize::MAX)
        });
        Ok(items)
    }

    async fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<MemoryItem>> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let wanted: std::collections::HashSet<&str> = tags.iter().map(|tag| tag.as_str()).collect();
        let mut items = Vec::new();
        for document in self.collection.iter().map_err(display_error)? {
            let document = document.map_err(display_error)?;
            let item = doc_to_item(&document)?;
            if item.tags.iter().any(|tag| wanted.contains(tag.as_str())) {
                items.push(item);
                if items.len() == limit {
                    break;
                }
            }
        }
        Ok(items)
    }

    async fn get_recent(&self, limit: usize) -> Result<Vec<MemoryItem>> {
        let mut items = Vec::new();
        for document in self.collection.iter().map_err(display_error)? {
            items.push(doc_to_item(&document.map_err(display_error)?)?);
        }
        items.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
        items.truncate(limit);
        Ok(items)
    }

    async fn get_important(&self, threshold: f32, limit: usize) -> Result<Vec<MemoryItem>> {
        let mut items = Vec::new();
        for document in self.collection.iter().map_err(display_error)? {
            let item = doc_to_item(&document.map_err(display_error)?)?;
            if item.importance >= threshold {
                items.push(item);
            }
        }
        items.sort_by(|left, right| {
            right
                .importance
                .partial_cmp(&left.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        items.truncate(limit);
        Ok(items)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = self.collection.delete(&[id]).map_err(display_error)?;
        if result.error_count != 0 {
            return Err(anyhow!("a3s-vec memory delete rejected {id}"));
        }
        Ok(())
    }

    async fn clear(&self) -> Result<()> {
        let mut ids = Vec::new();
        for document in self.collection.iter().map_err(display_error)? {
            ids.push(memory_id(&document.map_err(display_error)?)?);
        }
        if ids.is_empty() {
            return Ok(());
        }
        let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let result = self.collection.delete(&refs).map_err(display_error)?;
        if result.error_count != 0 {
            return Err(anyhow!(
                "a3s-vec memory clear rejected {} document(s)",
                result.error_count
            ));
        }
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        self.collection.count().map_err(display_error)
    }
}
