use super::GraphEventRecord;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

const MAX_GRAPH_EVENT_LOG_BYTES: usize = 256 * 1024 * 1024;

#[async_trait]
pub trait GraphEventStore: Send + Sync {
    /// Atomically replace one branch generation with a complete event log.
    async fn save(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()>;
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
}

#[async_trait]
impl GraphEventStore for FileGraphEventStore {
    async fn save(&self, branch_id: &str, events: &[GraphEventRecord]) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        tokio::fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("create graph event store `{}`", self.root.display()))?;
        let path = self.path(branch_id);
        let temp = Self::temp_path(&path);
        let bytes = serde_json::to_vec(events).context("serialize graph event log")?;
        if bytes.len() > MAX_GRAPH_EVENT_LOG_BYTES {
            anyhow::bail!(
                "graph event log is {} bytes; limit is {} bytes",
                bytes.len(),
                MAX_GRAPH_EVENT_LOG_BYTES
            );
        }
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

    async fn load(&self, branch_id: &str) -> Result<Option<Vec<GraphEventRecord>>> {
        let path = self.path(branch_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read graph event log `{}`", path.display()))
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

    async fn delete(&self, branch_id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_file(self.path(branch_id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("delete graph event log"),
        }
    }
}
