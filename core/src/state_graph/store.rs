use super::{GraphEventRecord, GraphRuntime};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

const MAX_GRAPH_EVENT_LOG_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphSaveOutcome {
    Saved,
    Conflict { actual_head: Option<String> },
}

pub fn graph_event_head(events: &[GraphEventRecord]) -> Option<&str> {
    events.last().map(|record| record.record_hash.as_str())
}

#[async_trait]
pub trait GraphEventStore: Send + Sync {
    /// Atomically replace one branch generation. Intended for a single writer.
    async fn save(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()>;
    /// Compare the persisted head and publish an extending generation atomically.
    async fn save_if_head(
        &self,
        _branch_id: &str,
        _expected_head: Option<&str>,
        _events: &[GraphEventRecord],
    ) -> Result<GraphSaveOutcome> {
        anyhow::bail!("graph event store does not support compare-and-swap")
    }
    async fn load(&self, branch_id: &str) -> Result<Option<Vec<GraphEventRecord>>>;
    async fn delete(&self, branch_id: &str) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct MemoryGraphEventStore {
    branches: RwLock<HashMap<String, Vec<GraphEventRecord>>>,
}

impl MemoryGraphEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl GraphEventStore for MemoryGraphEventStore {
    async fn save(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()> {
        self.branches
            .write()
            .await
            .insert(branch_id.to_string(), events.to_vec());
        Ok(())
    }

    async fn save_if_head(
        &self,
        branch_id: &str,
        expected_head: Option<&str>,
        events: &[GraphEventRecord],
    ) -> Result<GraphSaveOutcome> {
        validate_graph_events(events)?;
        let mut branches = self.branches.write().await;
        let current = branches.get(branch_id);
        let actual_head = current.and_then(|records| graph_event_head(records));
        if actual_head != expected_head {
            return Ok(GraphSaveOutcome::Conflict {
                actual_head: actual_head.map(str::to_string),
            });
        }
        ensure_extends(current.map(Vec::as_slice), events)?;
        branches.insert(branch_id.to_string(), events.to_vec());
        Ok(GraphSaveOutcome::Saved)
    }

    async fn load(&self, branch_id: &str) -> Result<Option<Vec<GraphEventRecord>>> {
        Ok(self.branches.read().await.get(branch_id).cloned())
    }

    async fn delete(&self, branch_id: &str) -> Result<()> {
        self.branches.write().await.remove(branch_id);
        Ok(())
    }
}

#[derive(Debug)]
pub struct FileGraphEventStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl FileGraphEventStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            write_lock: Mutex::new(()),
        }
    }

    fn path(&self, branch_id: &str) -> PathBuf {
        self.root
            .join(format!("{}.json", sha256::digest(branch_id.as_bytes())))
    }

    fn temp_path(path: &Path) -> PathBuf {
        path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()))
    }

    async fn acquire_process_lock(&self) -> Result<std::fs::File> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("create graph event store `{}`", self.root.display()))?;
        let path = self.root.join(".graph-store.lock");
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("open graph store lock `{}`", path.display()))?;
            file.lock_exclusive()
                .with_context(|| format!("lock graph store `{}`", path.display()))?;
            Ok(file)
        })
        .await
        .context("graph store lock task failed")?
    }

    async fn load_unlocked(&self, branch_id: &str) -> Result<Option<Vec<GraphEventRecord>>> {
        read_graph_events(&self.path(branch_id)).await
    }

    async fn publish_unlocked(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()> {
        let path = self.path(branch_id);
        let temp = Self::temp_path(&path);
        let bytes = encode_graph_events(events)?;
        let result = async {
            let mut file = tokio::fs::File::create(&temp)
                .await
                .with_context(|| format!("create graph event log `{}`", temp.display()))?;
            file.write_all(&bytes)
                .await
                .context("write graph event log")?;
            file.sync_all().await.context("sync graph event log")?;
            drop(file);
            let temp = temp.clone();
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                tempfile::TempPath::try_from_path(temp)?
                    .persist(path)
                    .map_err(|error| error.error)
            })
            .await
            .context("atomic graph event replace task failed")??;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        result
    }
}

#[async_trait]
impl GraphEventStore for FileGraphEventStore {
    async fn save(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let _process_guard = self.acquire_process_lock().await?;
        self.publish_unlocked(branch_id, events).await
    }

    async fn save_if_head(
        &self,
        branch_id: &str,
        expected_head: Option<&str>,
        events: &[GraphEventRecord],
    ) -> Result<GraphSaveOutcome> {
        validate_graph_events(events)?;
        let _guard = self.write_lock.lock().await;
        let _process_guard = self.acquire_process_lock().await?;
        let current = self.load_unlocked(branch_id).await?;
        let actual_head = current.as_deref().and_then(graph_event_head);
        if actual_head != expected_head {
            return Ok(GraphSaveOutcome::Conflict {
                actual_head: actual_head.map(str::to_string),
            });
        }
        ensure_extends(current.as_deref(), events)?;
        self.publish_unlocked(branch_id, events).await?;
        Ok(GraphSaveOutcome::Saved)
    }

    async fn load(&self, branch_id: &str) -> Result<Option<Vec<GraphEventRecord>>> {
        self.load_unlocked(branch_id).await
    }

    async fn delete(&self, branch_id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let _process_guard = self.acquire_process_lock().await?;
        match tokio::fs::remove_file(self.path(branch_id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("delete graph event log"),
        }
    }
}

fn validate_graph_events(events: &[GraphEventRecord]) -> Result<()> {
    GraphRuntime::strict_replay(events).context("validate graph event generation")?;
    Ok(())
}

fn ensure_extends(
    current: Option<&[GraphEventRecord]>,
    candidate: &[GraphEventRecord],
) -> Result<()> {
    let Some(current) = current else {
        return Ok(());
    };
    if candidate.len() < current.len() || candidate[..current.len()] != *current {
        anyhow::bail!("candidate graph generation does not extend the persisted history");
    }
    Ok(())
}

fn encode_graph_events(events: &[GraphEventRecord]) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(events).context("serialize graph event log")?;
    if bytes.len() > MAX_GRAPH_EVENT_LOG_BYTES {
        anyhow::bail!(
            "graph event log is {} bytes; limit is {} bytes",
            bytes.len(),
            MAX_GRAPH_EVENT_LOG_BYTES
        );
    }
    Ok(bytes)
}

async fn read_graph_events(path: &Path) -> Result<Option<Vec<GraphEventRecord>>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read graph event log `{}`", path.display()))
        }
    };
    if bytes.len() > MAX_GRAPH_EVENT_LOG_BYTES {
        anyhow::bail!(
            "graph event log `{}` is {} bytes; limit is {} bytes",
            path.display(),
            bytes.len(),
            MAX_GRAPH_EVENT_LOG_BYTES
        );
    }
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode graph event log `{}`", path.display()))
        .map(Some)
}
