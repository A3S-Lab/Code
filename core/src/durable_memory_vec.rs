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
    /// Set when an existing collection already has one non-null fp32
    /// `embedding` column. Inserts fill it with zeros and recall stays FTS.
    embedding_dimension: Option<usize>,
    /// Same-process exclusion. The engine file lock does not stop a second open here.
    /// Held so `Drop` releases the directory; the field itself is not read.
    _open_lease: OpenLease,
}

static OPEN_DIRECTORIES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

struct OpenLease(PathBuf);

impl Drop for OpenLease {
    fn drop(&mut self) {
        if let Ok(mut open) = OPEN_DIRECTORIES.lock() {
            open.remove(&self.0);
        }
    }
}

fn register_open_directory(path: PathBuf) -> Result<OpenLease> {
    let mut open = OPEN_DIRECTORIES
        .lock()
        .map_err(|_| anyhow!("memory open registry is poisoned"))?;
    if !open.insert(path.clone()) {
        return Err(anyhow!(
            "durable memory collection is already open at {}",
            path.display()
        ));
    }
    Ok(OpenLease(path))
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

/// `None` is a full-text collection this store created. `Some` is an existing
/// collection with one fp32 `embedding` column. Any other shape is refused
/// before this store writes.
fn accepted_embedding_dimension(collection: &Collection) -> Result<Option<usize>> {
    let schema = collection.schema().map_err(display_error)?;
    let mut names: Vec<&str> = schema.fields.iter().map(FieldSchema::name).collect();
    names.sort_unstable();
    const EXPECTED: [&str; 6] = [
        "content",
        "importance",
        "memory_type",
        "meta",
        "tags",
        "timestamp",
    ];
    if names != EXPECTED {
        return Err(anyhow!(
            "memory collection scalar fields do not match the durable memory set"
        ));
    }
    match schema.vectors() {
        [] => Ok(None),
        [vector]
            if vector.name() == "embedding"
                && vector.data_type() == DataType::VectorFp32
                && vector.dimension() > 0 =>
        {
            Ok(Some(vector.dimension() as usize))
        }
        _ => Err(anyhow!(
            "memory collection vector fields must be empty or one fp32 embedding column"
        )),
    }
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
        let canonical = root
            .canonicalize()
            .map_err(|error| anyhow!("canonicalize memory root {}: {error}", root.display()))?;
        // Register before opening. A refused second open must not close a file
        // descriptor that would release the first store's engine lock.
        let open_lease = register_open_directory(canonical)?;
        let collection_path = root.join(COLLECTION_NAME);
        let collection_path = collection_path
            .to_str()
            .ok_or_else(|| anyhow!("memory collection path is not UTF-8"))?;
        let (collection, embedding_dimension) = if collection_path_exists(collection_path) {
            let collection = Collection::open(collection_path, None).map_err(display_error)?;
            let embedding_dimension = accepted_embedding_dimension(&collection)?;
            (collection, embedding_dimension)
        } else {
            let schema = memory_schema()?;
            let collection = Collection::create_and_open(collection_path, &schema, None)
                .map_err(display_error)?;
            (collection, None)
        };
        Ok(Self {
            collection,
            path: collection_path.into(),
            embedding_dimension,
            _open_lease: open_lease,
        })
    }

    /// Insert or replace one memory record by id.
    pub fn put(&self, record: &MemoryRecord) -> Result<()> {
        let mut doc = record_to_doc(record)?;
        if let Some(dimension) = self.embedding_dimension {
            let zeros = vec![0.0_f32; dimension];
            doc.add_vector_f32("embedding", &zeros)
                .map_err(display_error)?;
        }
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

    /// One-time import of `items/*.json` written by `FileMemoryStore`.
    /// An id already in the collection is skipped. A file that cannot be read
    /// or parsed is an error after the readable files are stored, so the
    /// caller can leave the directory in place.
    pub fn import_legacy_json_items(&self, legacy_items_dir: &Path) -> Result<usize> {
        let entries = match std::fs::read_dir(legacy_items_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(anyhow!(
                    "read legacy memory items {}: {error}",
                    legacy_items_dir.display()
                ))
            }
        };
        let mut existing = std::collections::HashSet::new();
        for document in self.collection.iter().map_err(display_error)? {
            existing.insert(memory_id(&document.map_err(display_error)?)?);
        }
        let mut imported = 0usize;
        let mut complete = true;
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                complete = false;
                continue;
            };
            let Ok(item) = serde_json::from_str::<MemoryItem>(&raw) else {
                complete = false;
                continue;
            };
            if existing.contains(item.id.as_str()) {
                continue;
            }
            self.put(&item_to_record(&item)?)?;
            existing.insert(item.id);
            imported += 1;
        }
        if !complete {
            return Err(anyhow!(
                "skipped unreadable legacy memory files in {}; imported {imported}",
                legacy_items_dir.display()
            ));
        }
        Ok(imported)
    }
}

/// Opens the durable a3s-vec collection on the first memory operation.
pub struct LazyFileMemoryStore {
    directory: PathBuf,
    store: tokio::sync::OnceCell<VecMemoryStore>,
}

impl LazyFileMemoryStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            store: tokio::sync::OnceCell::new(),
        }
    }

    async fn inner(&self) -> Result<&VecMemoryStore> {
        let directory = self.directory.clone();
        self.store
            .get_or_try_init(|| async move {
                let store = VecMemoryStore::open(&directory)?;
                let legacy_items = directory.join("items");
                let legacy_index = directory.join("index.json");
                if legacy_items.is_dir() {
                    match store.import_legacy_json_items(&legacy_items) {
                        Ok(imported) if imported > 0 => {
                            tracing::info!(
                                imported,
                                dir = %directory.display(),
                                "imported legacy JSON memories into a3s-vec"
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "legacy memory import failed; legacy files kept"
                            );
                            return Ok::<VecMemoryStore, anyhow::Error>(store);
                        }
                    }
                    let _ = std::fs::rename(&legacy_items, directory.join("items.imported"));
                    if legacy_index.is_file() {
                        let _ =
                            std::fs::rename(&legacy_index, directory.join("index.json.imported"));
                    }
                }
                Ok::<VecMemoryStore, anyhow::Error>(store)
            })
            .await
            .map_err(|error| {
                anyhow!(
                    "failed to initialize a3s-vec Memory store at {}: {error:#}",
                    self.directory.display()
                )
            })
    }

    #[cfg(test)]
    fn is_initialized(&self) -> bool {
        self.store.get().is_some()
    }
}

#[async_trait::async_trait]
impl MemoryStore for LazyFileMemoryStore {
    async fn store(&self, item: MemoryItem) -> Result<()> {
        MemoryStore::store(self.inner().await?, item).await
    }

    async fn store_and_return(&self, item: MemoryItem) -> Result<MemoryItem> {
        MemoryStore::store_and_return(self.inner().await?, item).await
    }

    async fn retrieve(&self, id: &str) -> Result<Option<MemoryItem>> {
        MemoryStore::retrieve(self.inner().await?, id).await
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryItem>> {
        MemoryStore::search(self.inner().await?, query, limit).await
    }

    async fn search_by_tags(&self, tags: &[String], limit: usize) -> Result<Vec<MemoryItem>> {
        MemoryStore::search_by_tags(self.inner().await?, tags, limit).await
    }

    async fn get_recent(&self, limit: usize) -> Result<Vec<MemoryItem>> {
        MemoryStore::get_recent(self.inner().await?, limit).await
    }

    async fn get_important(&self, threshold: f32, limit: usize) -> Result<Vec<MemoryItem>> {
        MemoryStore::get_important(self.inner().await?, threshold, limit).await
    }

    async fn delete(&self, id: &str) -> Result<()> {
        MemoryStore::delete(self.inner().await?, id).await
    }

    async fn clear(&self) -> Result<()> {
        MemoryStore::clear(self.inner().await?).await
    }

    async fn count(&self) -> Result<usize> {
        MemoryStore::count(self.inner().await?).await
    }

    async fn prune(&self, policy: &a3s_memory::PrunePolicy) -> Result<usize> {
        MemoryStore::prune(self.inner().await?, policy).await
    }
}

fn fs_err_create_dir_all(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)
        .map_err(|error| anyhow!("create memory root {}: {error}", root.display()))?;
    crate::secure_state::restrict_state_dir(root)
        .map_err(|error| anyhow!("restrict memory root {}: {error}", root.display()))
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

    #[tokio::test]
    async fn lazy_store_opens_on_first_search_and_imports_legacy_json_once() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        std::fs::create_dir_all(memory_dir.join("items")).unwrap();
        let item = MemoryItem::new("legacy memory about e2e fixtures");
        std::fs::write(
            memory_dir.join(format!("items/{}.json", item.id)),
            serde_json::to_vec(&item).unwrap(),
        )
        .unwrap();

        let store = LazyFileMemoryStore::new(&memory_dir);
        assert!(!store.is_initialized());
        assert!(!memory_dir.join("durable_memory").exists());

        let matches = MemoryStore::search(&store, "legacy e2e fixtures", 5)
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].content.contains("legacy memory"));
        assert!(store.is_initialized());
        assert!(memory_dir.join("durable_memory").exists());
        assert!(memory_dir.join("items.imported").is_dir());
        assert!(!memory_dir.join("items").exists());

        drop(store);
        let store = LazyFileMemoryStore::new(&memory_dir);
        assert!(!store.is_initialized());
        assert_eq!(MemoryStore::count(&store).await.unwrap(), 1);
        assert!(!memory_dir.join("items").exists());
    }

    #[tokio::test]
    async fn a_skipped_legacy_file_keeps_the_items_directory() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let items = memory_dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let item = MemoryItem::new("legacy memory about e2e fixtures");
        std::fs::write(
            items.join(format!("{}.json", item.id)),
            serde_json::to_vec(&item).unwrap(),
        )
        .unwrap();
        std::fs::write(items.join("broken.json"), b"not-json").unwrap();

        let store = LazyFileMemoryStore::new(&memory_dir);
        let matches = MemoryStore::search(&store, "legacy e2e fixtures", 5)
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert!(items.join("broken.json").is_file());
        assert!(!memory_dir.join("items.imported").exists());

        drop(store);
        let store = LazyFileMemoryStore::new(&memory_dir);
        assert_eq!(MemoryStore::count(&store).await.unwrap(), 1);
        assert!(items.join("broken.json").is_file());
        assert!(!memory_dir.join("items.imported").exists());
    }

    #[test]
    fn an_existing_embedding_column_keeps_its_dimension() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let collection_path = create_embedding_collection(&memory_dir, 4);
        let store = VecMemoryStore::open(&memory_dir).unwrap();
        store
            .put(&record("m1", "legacy memory about e2e fixtures", 0.5))
            .unwrap();
        let hits = store.recall("legacy e2e fixtures", 5).unwrap();
        assert_eq!(hits.len(), 1);
        drop(store);

        let collection = a3s_vec::Collection::open(&collection_path, None).unwrap();
        let schema = collection.schema().unwrap();
        assert_eq!(schema.vectors().len(), 1);
        assert_eq!(schema.vectors()[0].name(), "embedding");
        assert_eq!(schema.vectors()[0].dimension(), 4);
    }

    #[test]
    fn a_new_collection_has_no_embedding_column() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        drop(VecMemoryStore::open(&memory_dir).unwrap());
        let collection_path = memory_dir.join(COLLECTION_NAME);
        let collection =
            a3s_vec::Collection::open(collection_path.to_str().unwrap(), None).unwrap();
        assert!(collection.schema().unwrap().vectors().is_empty());
    }

    #[test]
    fn a_foreign_schema_refuses_the_embedding_column() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let _path = create_named_vector_collection(&memory_dir, "other");
        let error = match VecMemoryStore::open(&memory_dir) {
            Ok(_) => panic!("foreign schema opened"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("embedding column"), "{error}");
        assert!(memory_dir.join(COLLECTION_NAME).exists());
    }

    #[test]
    fn a_second_open_is_refused_while_the_collection_is_already_open() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let first = VecMemoryStore::open(&memory_dir).unwrap();
        let error = match VecMemoryStore::open(&memory_dir) {
            Ok(_) => panic!("second open succeeded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("already open"), "{error}");
        drop(first);
        drop(VecMemoryStore::open(&memory_dir).unwrap());
    }

    #[tokio::test]
    async fn lazy_store_writes_and_recalls_an_existing_embedding_column() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let collection_path = create_embedding_collection(&memory_dir, 4);
        let store = LazyFileMemoryStore::new(&memory_dir);
        MemoryStore::store(&store, MemoryItem::new("legacy memory about e2e fixtures"))
            .await
            .unwrap();
        let matches = MemoryStore::search(&store, "legacy e2e fixtures", 5)
            .await
            .unwrap();
        assert!(
            matches
                .iter()
                .any(|item| item.content.contains("legacy memory")),
            "{matches:?}"
        );
        drop(store);

        let collection = a3s_vec::Collection::open(&collection_path, None).unwrap();
        let schema = collection.schema().unwrap();
        assert_eq!(schema.vectors().len(), 1);
        assert_eq!(schema.vectors()[0].name(), "embedding");
        assert_eq!(schema.vectors()[0].dimension(), 4);
    }

    #[tokio::test]
    async fn lazy_store_opens_without_an_embedding_column() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let store = LazyFileMemoryStore::new(&memory_dir);
        assert_eq!(MemoryStore::count(&store).await.unwrap(), 0);
        drop(store);
        let collection_path = memory_dir.join(COLLECTION_NAME);
        let collection =
            a3s_vec::Collection::open(collection_path.to_str().unwrap(), None).unwrap();
        assert!(collection.schema().unwrap().vectors().is_empty());
    }

    #[tokio::test]
    async fn a_foreign_schema_refuses_the_embedding_column_on_the_lazy_store() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        create_named_vector_collection(&memory_dir, "other");
        let store = LazyFileMemoryStore::new(&memory_dir);
        let error = match MemoryStore::count(&store).await {
            Ok(count) => panic!("foreign schema opened and counted {count}"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("embedding column"), "{error}");
        assert!(memory_dir.join(COLLECTION_NAME).exists());
    }

    #[tokio::test]
    async fn a_second_lazy_store_is_refused_while_already_open() {
        let root = tempfile::tempdir().unwrap();
        let memory_dir = root.path().join("memory");
        let first = LazyFileMemoryStore::new(&memory_dir);
        assert_eq!(MemoryStore::count(&first).await.unwrap(), 0);
        let second = LazyFileMemoryStore::new(&memory_dir);
        let error = match MemoryStore::count(&second).await {
            Ok(count) => panic!("second store opened and counted {count}"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("already open"), "{error}");
        drop(first);
        let third = LazyFileMemoryStore::new(&memory_dir);
        assert_eq!(MemoryStore::count(&third).await.unwrap(), 0);
    }

    fn create_embedding_collection(memory_dir: &std::path::Path, dimension: u32) -> String {
        create_named_vector_collection_with(memory_dir, "embedding", dimension)
    }

    fn create_named_vector_collection(memory_dir: &std::path::Path, name: &str) -> String {
        create_named_vector_collection_with(memory_dir, name, 4)
    }

    fn create_named_vector_collection_with(
        memory_dir: &std::path::Path,
        vector_name: &str,
        dimension: u32,
    ) -> String {
        std::fs::create_dir_all(memory_dir).unwrap();
        a3s_vec::initialize(None).unwrap();
        let collection_path = memory_dir.join(COLLECTION_NAME);
        let path = collection_path.to_str().unwrap().to_owned();
        let mut content = FieldSchema::new("content", DataType::String, false, 0).unwrap();
        content
            .set_index_params(&IndexParams::fts(Some("whitespace"), None, None).unwrap())
            .unwrap();
        let schema = CollectionSchema::builder(COLLECTION_NAME)
            .add_field(content)
            .add_field(FieldSchema::new("tags", DataType::String, true, 0).unwrap())
            .add_field(FieldSchema::new("importance", DataType::Float, true, 0).unwrap())
            .add_field(FieldSchema::new("memory_type", DataType::String, true, 0).unwrap())
            .add_field(FieldSchema::new("timestamp", DataType::String, true, 0).unwrap())
            .add_field(FieldSchema::new("meta", DataType::String, true, 0).unwrap())
            .add_vector_field(
                vector_name,
                DataType::VectorFp32,
                dimension,
                IndexParams::flat(a3s_vec::MetricType::Cosine).unwrap(),
            )
            .build()
            .unwrap();
        drop(a3s_vec::Collection::create_and_open(&path, &schema, None).unwrap());
        path
    }

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

    #[tokio::test]
    async fn search_over_many_records_returns_ranked_hits_without_scan_order() {
        // The id-mapping behind `search` runs on EVERY model prompt; it
        // once walked the whole collection. This pin holds the behavioral
        // contract of the primary-key fetch: the RIGHT hits, in FTS
        // ranking order, with unrelated records absent — across a store
        // large enough that a scan-order bug would surface as ordering
        // drift.
        use a3s_memory::MemoryStore as _;
        let root = tempfile::tempdir().unwrap();
        let store = VecMemoryStore::open(root.path()).unwrap();
        for index in 0..200 {
            let mut filler = MemoryItem::new(format!(
                "filler record {index} about harbor cranes and tide tables"
            ));
            filler.importance = 0.5;
            store.store(filler).await.unwrap();
        }
        for (id, content) in [
            ("m1", "the workspace prefers rust-analyzer navigation"),
            ("m2", "navigation preference: rust-analyzer over rls"),
            ("m3", "unrelated note about compiler flags"),
        ] {
            let mut item = MemoryItem::new(content);
            item.importance = 0.9;
            store.store(item).await.unwrap();
            let _ = id;
        }
        let hits = store
            .search("rust-analyzer navigation preference", 5)
            .await
            .unwrap();
        assert!(
            hits.len() >= 2,
            "both navigation memories must surface: {:?}",
            hits.iter()
                .map(|hit| hit.content.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            hits[0].content.contains("preference") || hits[0].content.contains("prefers"),
            "FTS ranking order preserved (the preference memories outrank filler): {:?}",
            hits.iter()
                .map(|hit| hit.content.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            hits.iter()
                .all(|hit| !hit.content.starts_with("filler record")),
            "filler must not crowd out the matches"
        );
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
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        // Primary-key fetch, not a collection scan: this runs on EVERY
        // model prompt (recall_similar), and the previous full `iter()`
        // made per-turn recall O(collection) to map ≤limit ids back to
        // documents. `fetch` keeps the FTS ranking via the re-sort below.
        // Vectors are skipped — recall needs fields only.
        let pks: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        let documents = self
            .collection
            .fetch_with_options(&pks, None, false)
            .map_err(display_error)?;
        let mut items = Vec::with_capacity(documents.len());
        for document in &documents {
            items.push(doc_to_item(document)?);
        }
        // Preserve FTS ranking order (fetch returns primary-key order).
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
        items.sort_by_key(|item| std::cmp::Reverse(item.timestamp));
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
