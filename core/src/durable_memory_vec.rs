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

use a3s_vec::{
    Collection, CollectionSchema, DataType, Doc, FieldSchema, IndexParams, SearchQuery,
};
use anyhow::{anyhow, Result};
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
    /// Unix epoch seconds.
    pub timestamp: i64,
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
        .add_field(
            FieldSchema::new("importance", DataType::Float, true, 0).map_err(display_error)?,
        )
        .add_field(
            FieldSchema::new("memory_type", DataType::String, true, 0).map_err(display_error)?,
        )
        .add_field(
            FieldSchema::new("timestamp", DataType::Int64, true, 0).map_err(display_error)?,
        )
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
    doc.add_i64("timestamp", record.timestamp)
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
        let collection_path =
            collection_path
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
        let topk = i32::try_from(limit)
            .map_err(|_| anyhow!("memory recall limit exceeds i32"))?;
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
            timestamp: 1_700_000_000,
        }
    }

    #[test]
    fn insert_and_fts_recall_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        store
            .put(&record("m1", "the workspace uses rust-analyzer for navigation", 0.8))
            .unwrap();
        store
            .put(&record("m2", "e2e fixtures live under tests/e2e/fixtures", 0.6))
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

    #[test]
    fn empty_query_returns_nothing() {
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        store.put(&record("m4", "something", 0.5)).unwrap();
        assert!(store.recall("   ", 5).unwrap().is_empty());
    }
}
