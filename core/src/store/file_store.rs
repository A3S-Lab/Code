use super::{SessionData, SessionSnapshotV1, SessionStore, SessionStoreCapabilities};
use crate::loop_checkpoint::LoopCheckpoint;
use crate::orchestration::WorkflowCheckpoint;
use crate::run::RunRecord;
use crate::subagent_task_tracker::SubagentTaskSnapshot;
use crate::tools::ArtifactStore;
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use anyhow::{Context, Result};
use base64::Engine as _;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Hard safety boundary for one JSON document owned by `FileSessionStore`.
///
/// The default artifact store is 16 MiB and resumed sessions may legitimately
/// exceed that after hosts raise artifact limits, so this remains deliberately
/// generous. It is still finite so an untrusted/corrupt file cannot force an
/// unbounded allocation before JSON and snapshot validation run.
pub(super) const MAX_FILE_STORE_JSON_BYTES: u64 = 256 * 1024 * 1024;

// ============================================================================
// File-based Session Store
// ============================================================================

/// File-based session store.
///
/// New saves store one complete [`SessionSnapshotV1`] JSON envelope per
/// session. Historical bare `SessionData` plus fragment directories remain
/// readable for migration.
/// ```text
/// sessions/
///   v1/
///     sessions/
///       id_<base64url-session-id>.json
///     loop_checkpoints/
///       id_<base64url-run-id>.json
///   session-1.json                 # legacy, read-only migration source
/// ```
pub struct FileSessionStore {
    /// Directory to store session files
    pub(super) dir: PathBuf,
}

impl FileSessionStore {
    /// Create a new file session store
    ///
    /// Creates the directory if it doesn't exist.
    pub async fn new<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("Failed to create session directory: {}", dir.display()))?;

        Ok(Self { dir })
    }

    fn encoded_path(&self, category: &str, id: &str) -> PathBuf {
        self.dir
            .join("v1")
            .join(category)
            .join(format!("{}.json", encoded_storage_key(id)))
    }

    fn encoded_dir(&self, category: &str, id: &str) -> PathBuf {
        self.dir
            .join("v1")
            .join(category)
            .join(encoded_storage_key(id))
    }

    /// Get the collision-free file path for a session.
    fn session_path(&self, id: &str) -> PathBuf {
        self.encoded_path("sessions", id)
    }

    fn legacy_session_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", legacy_safe_id(id)))
    }

    fn artifact_dir(&self, id: &str) -> PathBuf {
        self.encoded_dir("artifacts", id)
    }

    fn legacy_artifact_dir(&self, id: &str) -> PathBuf {
        self.dir.join("artifacts").join(legacy_safe_id(id))
    }

    fn trace_path(&self, id: &str) -> PathBuf {
        self.encoded_path("traces", id)
    }

    fn legacy_trace_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("traces")
            .join(format!("{}.json", legacy_safe_id(id)))
    }

    fn verification_path(&self, id: &str) -> PathBuf {
        self.encoded_path("verification", id)
    }

    fn legacy_verification_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("verification")
            .join(format!("{}.json", legacy_safe_id(id)))
    }

    fn runs_path(&self, id: &str) -> PathBuf {
        self.encoded_path("runs", id)
    }

    fn legacy_runs_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("runs")
            .join(format!("{}.json", legacy_safe_id(id)))
    }

    fn subagent_tasks_path(&self, id: &str) -> PathBuf {
        self.encoded_path("subagent_tasks", id)
    }

    fn legacy_subagent_tasks_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("subagent_tasks")
            .join(format!("{}.json", legacy_safe_id(id)))
    }

    fn loop_checkpoint_path(&self, run_id: &str) -> PathBuf {
        self.encoded_path("loop_checkpoints", run_id)
    }

    fn legacy_loop_checkpoint_path(&self, run_id: &str) -> PathBuf {
        self.dir
            .join("loop_checkpoints")
            .join(format!("{}.json", legacy_safe_id(run_id)))
    }

    fn workflow_checkpoint_path(&self, workflow_id: &str) -> PathBuf {
        self.encoded_path("workflow_checkpoints", workflow_id)
    }

    fn legacy_workflow_checkpoint_path(&self, workflow_id: &str) -> PathBuf {
        self.dir
            .join("workflow_checkpoints")
            .join(format!("{}.json", legacy_safe_id(workflow_id)))
    }

    async fn read_loop_checkpoint_at(path: &Path) -> Result<LoopCheckpoint> {
        let json = read_json_document(path, "loop checkpoint").await?;
        let checkpoint: LoopCheckpoint = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse loop checkpoint from {}", path.display()))?;
        checkpoint.ensure_loadable()?;
        Ok(checkpoint)
    }

    async fn read_workflow_checkpoint_at(path: &Path) -> Result<WorkflowCheckpoint> {
        let json = read_json_document(path, "workflow checkpoint").await?;
        let checkpoint: WorkflowCheckpoint = serde_json::from_slice(&json).with_context(|| {
            format!(
                "Failed to parse workflow checkpoint from {}",
                path.display()
            )
        })?;
        checkpoint.ensure_loadable()?;
        Ok(checkpoint)
    }

    async fn read_session_file(&self, id: &str) -> Result<Option<StoredSessionFile>> {
        let current = self.session_path(id);
        let legacy = self.legacy_session_path(id);
        let path = if current.exists() {
            current
        } else if legacy.exists() {
            legacy
        } else {
            return Ok(None);
        };

        let stored = Self::read_session_file_at(&path).await?;
        if stored.session_id() != id {
            anyhow::bail!(
                "session file key collision: requested id {:?}, but {} contains id {:?}",
                id,
                path.display(),
                stored.session_id()
            );
        }
        Ok(Some(stored))
    }

    async fn read_session_file_at(path: &Path) -> Result<StoredSessionFile> {
        let json = read_json_document(path, "session file").await?;
        let value: serde_json::Value = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse session file: {}", path.display()))?;

        // Once the document looks like an aggregate envelope, malformed or
        // future snapshots are errors. Never reinterpret them as legacy data.
        if value.get("schema_version").is_some() || value.get("session").is_some() {
            let snapshot: SessionSnapshotV1 = serde_json::from_value(value)
                .with_context(|| format!("Failed to parse session snapshot: {}", path.display()))?;
            snapshot
                .ensure_loadable()
                .with_context(|| format!("Session snapshot is not loadable: {}", path.display()))?;
            return Ok(StoredSessionFile::Snapshot(snapshot));
        }

        let session = serde_json::from_value(value)
            .with_context(|| format!("Failed to parse legacy session file: {}", path.display()))?;
        Ok(StoredSessionFile::Legacy(session))
    }

    async fn legacy_session_belongs_to(&self, id: &str) -> Result<bool> {
        let path = self.legacy_session_path(id);
        if !path.exists() {
            return Ok(false);
        }
        Ok(Self::read_session_file_at(&path).await?.session_id() == id)
    }

    async fn readable_component_path(
        &self,
        id: &str,
        current: PathBuf,
        legacy: PathBuf,
    ) -> Result<Option<PathBuf>> {
        if current.exists() {
            return Ok(Some(current));
        }
        if legacy.exists() && self.legacy_session_belongs_to(id).await? {
            return Ok(Some(legacy));
        }
        Ok(None)
    }
}

enum StoredSessionFile {
    Snapshot(SessionSnapshotV1),
    Legacy(SessionData),
}

impl StoredSessionFile {
    fn session_id(&self) -> &str {
        match self {
            Self::Snapshot(snapshot) => &snapshot.session.id,
            Self::Legacy(session) => &session.id,
        }
    }
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_suffix() -> String {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}.{}.{}", nanos, std::process::id(), counter)
}

fn encoded_storage_key(id: &str) -> String {
    format!(
        "id_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id.as_bytes())
    )
}

fn decode_storage_key(key: &str) -> Option<String> {
    let encoded = key.strip_prefix("id_")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn legacy_safe_id(id: &str) -> String {
    id.replace(['/', '\\'], "_").replace("..", "_")
}

async fn read_json_document(path: &Path, description: &str) -> Result<Vec<u8>> {
    let file = fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open {description}: {}", path.display()))?;
    let declared_len = file
        .metadata()
        .await
        .with_context(|| format!("Failed to inspect {description}: {}", path.display()))?
        .len();
    if declared_len > MAX_FILE_STORE_JSON_BYTES {
        anyhow::bail!(
            "Refusing to read {description} from {}: {} bytes exceeds the {} byte limit",
            path.display(),
            declared_len,
            MAX_FILE_STORE_JSON_BYTES
        );
    }

    // Re-check through a limited reader so a file that grows after the
    // metadata call cannot race past the boundary.
    let mut reader = file.take(MAX_FILE_STORE_JSON_BYTES + 1);
    let mut bytes = Vec::with_capacity(
        usize::try_from(declared_len)
            .unwrap_or(usize::MAX)
            .min(1024 * 1024),
    );
    reader
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("Failed to read {description}: {}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_STORE_JSON_BYTES {
        anyhow::bail!(
            "Refusing to read {description} from {}: document exceeds the {} byte limit",
            path.display(),
            MAX_FILE_STORE_JSON_BYTES
        );
    }
    Ok(bytes)
}

async fn write_json_atomic<T: serde::Serialize + ?Sized>(
    path: &Path,
    value: &T,
    description: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(value)
        .with_context(|| format!("Failed to serialize {description}"))?;
    if json.len() as u64 > MAX_FILE_STORE_JSON_BYTES {
        anyhow::bail!(
            "Refusing to write {description}: {} bytes exceeds the {} byte limit",
            json.len(),
            MAX_FILE_STORE_JSON_BYTES
        );
    }
    let temp_path = path.with_extension(format!("json.{}.tmp", unique_temp_suffix()));

    let result = async {
        let mut file = fs::File::create(&temp_path)
            .await
            .with_context(|| format!("Failed to create temp file: {}", temp_path.display()))?;
        file.write_all(&json)
            .await
            .with_context(|| format!("Failed to write {description}"))?;
        file.sync_all()
            .await
            .with_context(|| format!("Failed to sync {description}"))?;
        // Windows cannot replace an existing destination with std/tokio
        // rename. TempPath::persist uses the platform's atomic replace
        // primitive (MoveFileExW on Windows, rename on Unix).
        drop(file);
        let temp_path = temp_path.clone();
        let target_path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            tempfile::TempPath::try_from_path(temp_path)?
                .persist(target_path)
                .map_err(|error| error.error)
        })
        .await
        .context("Atomic session replace task failed")?
        .with_context(|| {
            format!(
                "Failed to atomically replace {} with {}",
                description,
                path.display()
            )
        })?;
        Ok(())
    }
    .await;

    if result.is_err() {
        let _ = fs::remove_file(&temp_path).await;
    }
    result
}

async fn remove_file_if_exists(path: &Path, description: &str) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .await
            .with_context(|| format!("Failed to delete {description}: {}", path.display()))?;
    }
    Ok(())
}

async fn remove_dir_if_exists(path: &Path, description: &str) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .await
            .with_context(|| format!("Failed to delete {description}: {}", path.display()))?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        let path = self.session_path(&session.id);

        // Preserve aggregate components when a legacy caller updates only the
        // SessionData portion of an already-migrated record.
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) =
            self.read_session_file(&session.id).await?
        {
            snapshot.session = session.clone();
            return self.save_snapshot(&snapshot).await;
        }

        write_json_atomic(&path, session, &format!("session {}", session.id)).await?;

        tracing::debug!("Saved session {} to {}", session.id, path.display());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<SessionData>> {
        let session = match self.read_session_file(id).await? {
            Some(StoredSessionFile::Snapshot(snapshot)) => Some(snapshot.session),
            Some(StoredSessionFile::Legacy(session)) => Some(session),
            None => None,
        };
        if session.is_some() {
            tracing::debug!(
                "Loaded session {} from {}",
                id,
                self.session_path(id).display()
            );
        }
        Ok(session)
    }

    async fn save_snapshot(&self, snapshot: &SessionSnapshotV1) -> Result<()> {
        snapshot.ensure_loadable()?;
        let path = self.session_path(&snapshot.session.id);
        write_json_atomic(
            &path,
            snapshot,
            &format!("session snapshot {}", snapshot.session.id),
        )
        .await?;
        tracing::debug!(
            "Saved session snapshot {} to {}",
            snapshot.session.id,
            path.display()
        );
        Ok(())
    }

    async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
        match self.read_session_file(id).await? {
            Some(StoredSessionFile::Snapshot(snapshot)) => Ok(Some(snapshot)),
            Some(StoredSessionFile::Legacy(session)) => {
                let artifacts = self.load_artifacts(id).await?.unwrap_or_default();
                Ok(Some(SessionSnapshotV1::new(
                    session,
                    &artifacts,
                    self.load_trace_events(id).await?.unwrap_or_default(),
                    self.load_run_records(id).await?.unwrap_or_default(),
                    self.load_verification_reports(id)
                        .await?
                        .unwrap_or_default(),
                    self.load_subagent_tasks(id).await?.unwrap_or_default(),
                )))
            }
            None => Ok(None),
        }
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities {
            atomic_session_snapshots: true,
        }
    }

    async fn delete(&self, id: &str) -> Result<()> {
        // Only touch a legacy path when the document stored there proves that
        // it belongs to the requested id. Historical sanitization was lossy,
        // so path equality alone is not ownership evidence.
        let legacy_owned = self.legacy_session_belongs_to(id).await?;

        remove_file_if_exists(&self.session_path(id), "session file").await?;
        remove_dir_if_exists(&self.artifact_dir(id), "artifact directory").await?;
        remove_file_if_exists(&self.trace_path(id), "trace file").await?;
        remove_file_if_exists(&self.verification_path(id), "verification report file").await?;
        remove_file_if_exists(&self.runs_path(id), "run record file").await?;
        remove_file_if_exists(&self.subagent_tasks_path(id), "subagent task file").await?;

        if legacy_owned {
            remove_file_if_exists(&self.legacy_session_path(id), "legacy session file").await?;
            remove_dir_if_exists(&self.legacy_artifact_dir(id), "legacy artifact directory")
                .await?;
            remove_file_if_exists(&self.legacy_trace_path(id), "legacy trace file").await?;
            remove_file_if_exists(
                &self.legacy_verification_path(id),
                "legacy verification report file",
            )
            .await?;
            remove_file_if_exists(&self.legacy_runs_path(id), "legacy run record file").await?;
            remove_file_if_exists(
                &self.legacy_subagent_tasks_path(id),
                "legacy subagent task file",
            )
            .await?;
        }

        tracing::debug!("Deleted session {}", id);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        let mut session_ids = BTreeSet::new();

        let current_dir = self.dir.join("v1").join("sessions");
        if current_dir.exists() {
            let mut entries = fs::read_dir(&current_dir).await.with_context(|| {
                format!(
                    "Failed to read session directory: {}",
                    current_dir.display()
                )
            })?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    if let Some(id) = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .and_then(decode_storage_key)
                    {
                        session_ids.insert(id);
                    }
                }
            }
        }

        let mut entries = fs::read_dir(&self.dir)
            .await
            .with_context(|| format!("Failed to read session directory: {}", self.dir.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                match Self::read_session_file_at(&path).await {
                    Ok(stored) => {
                        session_ids.insert(stored.session_id().to_string());
                    }
                    Err(error) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %error,
                            "Skipping unreadable legacy session while listing"
                        );
                    }
                }
            }
        }

        Ok(session_ids.into_iter().collect())
    }

    async fn exists(&self, id: &str) -> Result<bool> {
        if self.session_path(id).exists() {
            return Ok(true);
        }
        self.legacy_session_belongs_to(id).await
    }

    async fn save_artifacts(&self, id: &str, artifacts: &ArtifactStore) -> Result<()> {
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) = self.read_session_file(id).await? {
            snapshot.artifacts = artifacts.artifacts();
            return self.save_snapshot(&snapshot).await;
        }

        let artifact_dir = self.artifact_dir(id);
        artifacts.save_to_dir(&artifact_dir).with_context(|| {
            format!(
                "Failed to save artifacts for session {} to {}",
                id,
                artifact_dir.display()
            )
        })
    }

    async fn load_artifacts(&self, id: &str) -> Result<Option<ArtifactStore>> {
        if let Some(StoredSessionFile::Snapshot(snapshot)) = self.read_session_file(id).await? {
            return Ok(Some(snapshot.artifact_store()));
        }

        let current = self.artifact_dir(id);
        let legacy = self.legacy_artifact_dir(id);
        let artifact_dir = if current.exists() {
            current
        } else if legacy.exists() && self.legacy_session_belongs_to(id).await? {
            legacy
        } else {
            return Ok(None);
        };
        if !artifact_dir.exists() {
            return Ok(None);
        }

        let artifacts = ArtifactStore::load_from_dir(&artifact_dir).with_context(|| {
            format!(
                "Failed to load artifacts for session {} from {}",
                id,
                artifact_dir.display()
            )
        })?;
        Ok(Some(artifacts))
    }

    async fn save_trace_events(&self, id: &str, events: &[TraceEvent]) -> Result<()> {
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) = self.read_session_file(id).await? {
            snapshot.trace_events = events.to_vec();
            return self.save_snapshot(&snapshot).await;
        }

        let path = self.trace_path(id);
        write_json_atomic(&path, events, &format!("trace events for session {id}")).await
    }

    async fn load_trace_events(&self, id: &str) -> Result<Option<Vec<TraceEvent>>> {
        if let Some(StoredSessionFile::Snapshot(snapshot)) = self.read_session_file(id).await? {
            return Ok(Some(snapshot.trace_events));
        }

        let Some(path) = self
            .readable_component_path(id, self.trace_path(id), self.legacy_trace_path(id))
            .await?
        else {
            return Ok(None);
        };

        let json = read_json_document(&path, "trace events").await?;
        let events = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse trace events from {}", path.display()))?;
        Ok(Some(events))
    }

    async fn save_run_records(&self, id: &str, records: &[RunRecord]) -> Result<()> {
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) = self.read_session_file(id).await? {
            snapshot.run_records = records.to_vec();
            return self.save_snapshot(&snapshot).await;
        }

        let path = self.runs_path(id);
        write_json_atomic(&path, records, &format!("run records for session {id}")).await
    }

    async fn load_run_records(&self, id: &str) -> Result<Option<Vec<RunRecord>>> {
        if let Some(StoredSessionFile::Snapshot(snapshot)) = self.read_session_file(id).await? {
            return Ok(Some(snapshot.run_records));
        }

        let Some(path) = self
            .readable_component_path(id, self.runs_path(id), self.legacy_runs_path(id))
            .await?
        else {
            return Ok(None);
        };

        let json = read_json_document(&path, "run records").await?;
        let records = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse run records from {}", path.display()))?;
        Ok(Some(records))
    }

    async fn save_verification_reports(
        &self,
        id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) = self.read_session_file(id).await? {
            snapshot.verification_reports = reports.to_vec();
            return self.save_snapshot(&snapshot).await;
        }

        let path = self.verification_path(id);
        write_json_atomic(
            &path,
            reports,
            &format!("verification reports for session {id}"),
        )
        .await
    }

    async fn load_verification_reports(&self, id: &str) -> Result<Option<Vec<VerificationReport>>> {
        if let Some(StoredSessionFile::Snapshot(snapshot)) = self.read_session_file(id).await? {
            return Ok(Some(snapshot.verification_reports));
        }

        let Some(path) = self
            .readable_component_path(
                id,
                self.verification_path(id),
                self.legacy_verification_path(id),
            )
            .await?
        else {
            return Ok(None);
        };

        let json = read_json_document(&path, "verification reports").await?;
        let reports = serde_json::from_slice(&json).with_context(|| {
            format!(
                "Failed to parse verification reports from {}",
                path.display()
            )
        })?;
        Ok(Some(reports))
    }

    async fn save_subagent_tasks(&self, id: &str, tasks: &[SubagentTaskSnapshot]) -> Result<()> {
        if let Some(StoredSessionFile::Snapshot(mut snapshot)) = self.read_session_file(id).await? {
            snapshot.subagent_tasks = tasks.to_vec();
            return self.save_snapshot(&snapshot).await;
        }

        let path = self.subagent_tasks_path(id);
        write_json_atomic(&path, tasks, &format!("subagent tasks for session {id}")).await
    }

    async fn load_subagent_tasks(&self, id: &str) -> Result<Option<Vec<SubagentTaskSnapshot>>> {
        if let Some(StoredSessionFile::Snapshot(snapshot)) = self.read_session_file(id).await? {
            return Ok(Some(snapshot.subagent_tasks));
        }

        let Some(path) = self
            .readable_component_path(
                id,
                self.subagent_tasks_path(id),
                self.legacy_subagent_tasks_path(id),
            )
            .await?
        else {
            return Ok(None);
        };
        let json = read_json_document(&path, "subagent tasks").await?;
        let tasks = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse subagent tasks from {}", path.display()))?;
        Ok(Some(tasks))
    }

    async fn save_loop_checkpoint(&self, run_id: &str, checkpoint: &LoopCheckpoint) -> Result<()> {
        checkpoint.ensure_addressed_by(run_id)?;
        let path = self.loop_checkpoint_path(run_id);
        write_json_atomic(
            &path,
            checkpoint,
            &format!("loop checkpoint for run {run_id}"),
        )
        .await
    }

    async fn load_loop_checkpoint(&self, run_id: &str) -> Result<Option<LoopCheckpoint>> {
        let current = self.loop_checkpoint_path(run_id);
        let legacy = self.legacy_loop_checkpoint_path(run_id);
        let path = if current.exists() {
            current
        } else if legacy.exists() {
            legacy
        } else {
            return Ok(None);
        };
        let checkpoint = Self::read_loop_checkpoint_at(&path).await?;
        checkpoint.ensure_addressed_by(run_id)?;
        Ok(Some(checkpoint))
    }

    async fn delete_loop_checkpoint(&self, run_id: &str) -> Result<()> {
        remove_file_if_exists(&self.loop_checkpoint_path(run_id), "loop checkpoint").await?;
        let legacy = self.legacy_loop_checkpoint_path(run_id);
        if legacy.exists() {
            let checkpoint = Self::read_loop_checkpoint_at(&legacy).await?;
            checkpoint.ensure_addressed_by(run_id)?;
            remove_file_if_exists(&legacy, "legacy loop checkpoint").await?;
        }
        Ok(())
    }

    async fn save_workflow_checkpoint(
        &self,
        workflow_id: &str,
        checkpoint: &WorkflowCheckpoint,
    ) -> Result<()> {
        if checkpoint.workflow_id != workflow_id {
            anyhow::bail!(
                "workflow checkpoint key mismatch: requested workflow {:?}, payload belongs to {:?}",
                workflow_id,
                checkpoint.workflow_id
            );
        }
        let path = self.workflow_checkpoint_path(workflow_id);
        write_json_atomic(
            &path,
            checkpoint,
            &format!("workflow checkpoint for {workflow_id}"),
        )
        .await
    }

    async fn load_workflow_checkpoint(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>> {
        let current = self.workflow_checkpoint_path(workflow_id);
        let legacy = self.legacy_workflow_checkpoint_path(workflow_id);
        let path = if current.exists() {
            current
        } else if legacy.exists() {
            legacy
        } else {
            return Ok(None);
        };
        let checkpoint = Self::read_workflow_checkpoint_at(&path).await?;
        if checkpoint.workflow_id != workflow_id {
            anyhow::bail!(
                "workflow checkpoint key mismatch: requested workflow {:?}, payload belongs to {:?}",
                workflow_id,
                checkpoint.workflow_id
            );
        }
        Ok(Some(checkpoint))
    }

    async fn delete_workflow_checkpoint(&self, workflow_id: &str) -> Result<()> {
        remove_file_if_exists(
            &self.workflow_checkpoint_path(workflow_id),
            "workflow checkpoint",
        )
        .await?;
        let legacy = self.legacy_workflow_checkpoint_path(workflow_id);
        if legacy.exists() {
            let checkpoint = Self::read_workflow_checkpoint_at(&legacy).await?;
            if checkpoint.workflow_id != workflow_id {
                anyhow::bail!(
                    "workflow checkpoint key mismatch: requested workflow {:?}, payload belongs to {:?}",
                    workflow_id,
                    checkpoint.workflow_id
                );
            }
            remove_file_if_exists(&legacy, "legacy workflow checkpoint").await?;
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        // Verify directory exists and is writable
        let probe = self.dir.join(".health_check");
        fs::write(&probe, b"ok")
            .await
            .with_context(|| format!("Store directory not writable: {}", self.dir.display()))?;
        let _ = fs::remove_file(&probe).await;
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "file"
    }
}
