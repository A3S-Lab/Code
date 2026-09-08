use super::{
    snapshot_content_digest, FileSessionStoreWal, SessionData, SessionSnapshotV1, SessionStore,
    SessionStoreAtRestCipher, SessionStoreCapabilities, SessionStoreCommitEventV1,
    SessionStoreCommitWatch, SessionStoreWalEntryV1, SessionStoreWalPhaseV1,
    SessionStoreWriterLeaseV1,
};
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
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, Mutex};

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
    pub(super) write_lock: Mutex<()>,
    pub(super) wal: FileSessionStoreWal,
    pub(super) next_wal_sequence: AtomicU64,
    /// Process-local copy of the last lease this handle acquired.
    pub(super) held_writer_lease: Mutex<Option<SessionStoreWriterLeaseV1>>,
    pub(super) commit_watch: broadcast::Sender<SessionStoreCommitEventV1>,
    pub(super) encryption: Option<SessionStoreAtRestCipher>,
}

impl FileSessionStore {
    /// Create a new file session store
    ///
    /// Creates the directory if it doesn't exist.
    pub async fn new<P: AsRef<Path>>(dir: P) -> Result<Self> {
        Self::new_with_encryption(dir, None).await
    }

    /// Open like [`Self::new`], but if the WAL has a duplicate sequence
    /// conflict (typically from concurrent writers without a shared lock),
    /// quarantine the corrupt log and reopen from durable session snapshots.
    ///
    /// Fail-closed [`Self::new`] remains the integrity default for tests and
    /// strict callers; Host TUI / CLI resume paths use this recovery entry.
    pub async fn new_recovering_corrupt_wal<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref();
        match Self::new(dir).await {
            Ok(store) => Ok(store),
            Err(error) if FileSessionStoreWal::is_sequence_conflict(&error) => {
                let quarantined = FileSessionStoreWal::new(dir).quarantine_corrupt().await?;
                tracing::warn!(
                    wal = %quarantined.display(),
                    "quarantined corrupt session store WAL; continuing from durable session snapshots"
                );
                Self::new(dir).await.with_context(|| {
                    format!(
                        "failed to reopen session store after quarantining corrupt WAL {}",
                        quarantined.display()
                    )
                })
            }
            Err(error) => Err(error),
        }
    }

    /// Create a file session store that encrypts durable documents at rest
    /// (STORE-ENCRYPT1). The digest-only WAL remains unencrypted.
    pub async fn with_encryption_key<P: AsRef<Path>>(dir: P, key: &[u8; 32]) -> Result<Self> {
        Self::new_with_encryption(dir, Some(SessionStoreAtRestCipher::new(key)?)).await
    }

    async fn new_with_encryption<P: AsRef<Path>>(
        dir: P,
        encryption: Option<SessionStoreAtRestCipher>,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("Failed to create session directory: {}", dir.display()))?;

        let (commit_watch, _) = super::watch::commit_watch_channel();
        let store = Self {
            dir: dir.clone(),
            write_lock: Mutex::new(()),
            wal: FileSessionStoreWal::new(&dir),
            next_wal_sequence: AtomicU64::new(1),
            held_writer_lease: Mutex::new(None),
            commit_watch,
            encryption,
        };
        // Cross-process lock so two `new()` + commit races cannot mint the
        // same WAL sequence (in-process AtomicU64 alone is not enough).
        let _wal_lock = store.acquire_wal_file_lock().await?;
        let next = store.recover_wal().await?;
        store.next_wal_sequence.store(next, Ordering::SeqCst);
        Ok(store)
    }

    fn wal_lock_path(&self) -> PathBuf {
        self.dir.join("v1").join("wal").join("session-store.lock")
    }

    /// Exclusive cross-process lock covering WAL recover and sequence minting.
    async fn acquire_wal_file_lock(&self) -> Result<std::fs::File> {
        let path = self.wal_lock_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "Failed to create session store WAL lock directory: {}",
                    parent.display()
                )
            })?;
        }
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| {
                    format!("Failed to open session store WAL lock {}", path.display())
                })?;
            file.lock_exclusive().with_context(|| {
                format!(
                    "Failed to lock session store WAL exclusively at {}",
                    path.display()
                )
            })?;
            Ok(file)
        })
        .await
        .context("session store WAL lock task failed")?
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Seal open WAL intents whose durable snapshot already matches, then
    /// return the next one-based WAL sequence.
    async fn recover_wal(&self) -> Result<u64> {
        let (entries, mut next_sequence) = self.wal.load_entries().await?;
        let mut open_intents = std::collections::BTreeMap::new();
        for entry in entries {
            match entry.phase {
                SessionStoreWalPhaseV1::Intent => {
                    open_intents.insert(entry.sequence, entry);
                }
                SessionStoreWalPhaseV1::Committed => {
                    open_intents.remove(&entry.sequence);
                }
            }
        }
        for (_, intent) in open_intents {
            let Some(snapshot) = self.load_snapshot_without_wal(&intent.session_id).await? else {
                continue;
            };
            let current = snapshot_content_digest(&snapshot)?;
            if current != intent.snapshot_digest {
                continue;
            }
            let committed = SessionStoreWalEntryV1::new(
                intent.sequence,
                intent.session_id,
                intent.snapshot_digest,
                SessionStoreWalPhaseV1::Committed,
                intent.recorded_at_ms.saturating_add(1),
            )?;
            self.wal.append(&committed).await?;
            next_sequence = next_sequence.max(committed.sequence + 1);
        }
        Ok(next_sequence)
    }

    /// Load a snapshot without depending on WAL recovery side effects.
    async fn load_snapshot_without_wal(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
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

    fn encoded_path(&self, category: &str, id: &str) -> PathBuf {
        self.dir
            .join("v1")
            .join(category)
            .join(format!("{}.json", encoded_storage_key(id)))
    }

    async fn write_json_atomic<T: serde::Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        description: &str,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        write_json_atomic(path, value, description, self.encryption.as_ref()).await
    }

    async fn write_json_atomic_unlocked<T: serde::Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        description: &str,
    ) -> Result<()> {
        write_json_atomic(path, value, description, self.encryption.as_ref()).await
    }

    async fn commit_snapshot_under_lock(&self, snapshot: &SessionSnapshotV1) -> Result<()> {
        // Held after `write_lock` so lock order is always process-mutex → flock.
        let _wal_lock = self.acquire_wal_file_lock().await?;
        self.ensure_writer_lease_allows_commit_unlocked().await?;
        snapshot.ensure_loadable()?;
        let snapshot_digest = snapshot_content_digest(snapshot)?;
        // Re-read durable max under the flock so a second process that committed
        // while this handle was idle cannot cause a duplicate sequence mint.
        let (_, next) = self.wal.load_entries().await?;
        self.next_wal_sequence.store(next, Ordering::SeqCst);
        let sequence = self.next_wal_sequence.fetch_add(1, Ordering::SeqCst);
        let recorded_at_ms = Self::now_ms();
        let intent = SessionStoreWalEntryV1::new(
            sequence,
            snapshot.session.id.clone(),
            snapshot_digest.clone(),
            SessionStoreWalPhaseV1::Intent,
            recorded_at_ms,
        )?;
        self.wal.append(&intent).await?;

        let path = self.session_path(&snapshot.session.id);
        self.write_json_atomic_unlocked(
            &path,
            snapshot,
            &format!("session snapshot {}", snapshot.session.id),
        )
        .await?;

        let committed = SessionStoreWalEntryV1::new(
            sequence,
            snapshot.session.id.clone(),
            snapshot_digest.clone(),
            SessionStoreWalPhaseV1::Committed,
            recorded_at_ms.saturating_add(1),
        )?;
        self.wal.append(&committed).await?;
        let event = SessionStoreCommitEventV1::new(
            snapshot.session.id.clone(),
            snapshot_digest,
            recorded_at_ms.saturating_add(1),
        )?;
        let _ = self.commit_watch.send(event);
        tracing::debug!(
            "Saved session snapshot {} to {}",
            snapshot.session.id,
            path.display()
        );
        Ok(())
    }

    async fn seal_artifact_manifest_if_needed(&self, artifact_dir: &Path) -> Result<()> {
        let Some(cipher) = &self.encryption else {
            return Ok(());
        };
        let path = artifact_dir.join("artifacts.json");
        if !path.exists() {
            return Ok(());
        }
        let plain = fs::read(&path)
            .await
            .with_context(|| format!("Failed to read artifact manifest {}", path.display()))?;
        if SessionStoreAtRestCipher::is_sealed(&plain) {
            return Ok(());
        }
        let sealed = cipher.seal(&plain)?;
        fs::write(&path, sealed)
            .await
            .with_context(|| format!("Failed to seal artifact manifest {}", path.display()))?;
        Ok(())
    }

    async fn load_artifact_manifest_bytes(&self, artifact_dir: &Path) -> Result<Vec<u8>> {
        let path = artifact_dir.join("artifacts.json");
        if !path.exists() {
            return Ok(b"{\"artifacts\":[]}".to_vec());
        }
        let bytes = fs::read(&path)
            .await
            .with_context(|| format!("Failed to read artifact manifest {}", path.display()))?;
        match &self.encryption {
            Some(cipher) => cipher.open_or_plaintext(&bytes),
            None if SessionStoreAtRestCipher::is_sealed(&bytes) => anyhow::bail!(
                "Refusing to read sealed artifact manifest from {} without an at-rest encryption key",
                path.display()
            ),
            None => Ok(bytes),
        }
    }

    fn writer_lease_path(&self) -> PathBuf {
        self.dir.join("v1").join("writer_lease.json")
    }

    async fn read_durable_writer_lease_unlocked(
        &self,
    ) -> Result<Option<SessionStoreWriterLeaseV1>> {
        let path = self.writer_lease_path();
        if !path.exists() {
            return Ok(None);
        }
        let json = read_json_document(&path, "writer lease", self.encryption.as_ref()).await?;
        let lease: SessionStoreWriterLeaseV1 = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse writer lease from {}", path.display()))?;
        lease.validate()?;
        Ok(Some(lease))
    }

    async fn ensure_writer_lease_allows_commit_unlocked(&self) -> Result<()> {
        let durable = self.read_durable_writer_lease_unlocked().await?;
        let held = self.held_writer_lease.lock().await.clone();
        match (durable, held) {
            (None, _) => Ok(()),
            (Some(durable), Some(held)) if durable.matches_holder(&held) => Ok(()),
            (Some(durable), _) => anyhow::bail!(
                "session store writer lease lost or taken over (durable epoch {})",
                durable.epoch
            ),
        }
    }

    async fn acquire_writer_lease_under_lock(
        &self,
        holder_id: &str,
    ) -> Result<SessionStoreWriterLeaseV1> {
        let current = self.read_durable_writer_lease_unlocked().await?;
        let epoch = current
            .map(|lease| lease.epoch)
            .unwrap_or(0)
            .saturating_add(1);
        let lease = SessionStoreWriterLeaseV1::new(epoch, holder_id, Self::now_ms())?;
        let path = self.writer_lease_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "Failed to create writer lease directory: {}",
                    parent.display()
                )
            })?;
        }
        self.write_json_atomic_unlocked(&path, &lease, "writer lease")
            .await?;
        *self.held_writer_lease.lock().await = Some(lease.clone());
        Ok(lease)
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

    async fn read_loop_checkpoint_at(
        path: &Path,
        encryption: Option<&SessionStoreAtRestCipher>,
    ) -> Result<LoopCheckpoint> {
        let json = read_json_document(path, "loop checkpoint", encryption).await?;
        let checkpoint: LoopCheckpoint = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse loop checkpoint from {}", path.display()))?;
        checkpoint.ensure_loadable()?;
        Ok(checkpoint)
    }

    async fn read_workflow_checkpoint_at(
        path: &Path,
        encryption: Option<&SessionStoreAtRestCipher>,
    ) -> Result<WorkflowCheckpoint> {
        let json = read_json_document(path, "workflow checkpoint", encryption).await?;
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

        let stored = Self::read_session_file_at(&path, self.encryption.as_ref()).await?;
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

    async fn read_session_file_at(
        path: &Path,
        encryption: Option<&SessionStoreAtRestCipher>,
    ) -> Result<StoredSessionFile> {
        let json = read_json_document(path, "session file", encryption).await?;
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
        Ok(Self::read_session_file_at(&path, self.encryption.as_ref())
            .await?
            .session_id()
            == id)
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

fn persist_temp_path(temp_path: PathBuf, target_path: PathBuf) -> std::io::Result<()> {
    let mut temp_path = tempfile::TempPath::try_from_path(temp_path)?;
    let mut failed_attempts = 0usize;

    loop {
        match temp_path.persist(&target_path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                failed_attempts += 1;
                let Some(delay) = windows_atomic_replace_retry_delay(&error.error, failed_attempts)
                else {
                    return Err(error.error);
                };
                temp_path = error.path;
                std::thread::sleep(delay);
            }
        }
    }
}

pub(super) fn windows_atomic_replace_retry_delay(
    error: &std::io::Error,
    failed_attempts: usize,
) -> Option<std::time::Duration> {
    #[cfg(windows)]
    {
        const MAX_ATTEMPTS: usize = 6;
        const BASE_DELAY_MS: u64 = 5;
        const ERROR_ACCESS_DENIED: i32 = 5;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;

        if failed_attempts >= MAX_ATTEMPTS
            || !matches!(
                error.raw_os_error(),
                Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            )
        {
            return None;
        }

        let exponent = u32::try_from(failed_attempts.saturating_sub(1))
            .unwrap_or(u32::MAX)
            .min(4);
        Some(std::time::Duration::from_millis(
            BASE_DELAY_MS * 2u64.pow(exponent),
        ))
    }

    #[cfg(not(windows))]
    {
        let _ = (error, failed_attempts);
        None
    }
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

async fn read_json_document(
    path: &Path,
    description: &str,
    encryption: Option<&SessionStoreAtRestCipher>,
) -> Result<Vec<u8>> {
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
    match encryption {
        Some(cipher) => cipher.open_or_plaintext(&bytes),
        None if SessionStoreAtRestCipher::is_sealed(&bytes) => {
            anyhow::bail!(
                "Refusing to read sealed {description} from {} without an at-rest encryption key",
                path.display()
            )
        }
        None => Ok(bytes),
    }
}

async fn write_json_atomic<T: serde::Serialize + ?Sized>(
    path: &Path,
    value: &T,
    description: &str,
    encryption: Option<&SessionStoreAtRestCipher>,
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
    let payload = match encryption {
        Some(cipher) => cipher.seal(&json)?,
        None => json,
    };
    if payload.len() as u64 > MAX_FILE_STORE_JSON_BYTES {
        anyhow::bail!(
            "Refusing to write {description}: sealed {} bytes exceeds the {} byte limit",
            payload.len(),
            MAX_FILE_STORE_JSON_BYTES
        );
    }
    let temp_path = path.with_extension(format!("json.{}.tmp", unique_temp_suffix()));

    let result = async {
        let mut file = fs::File::create(&temp_path)
            .await
            .with_context(|| format!("Failed to create temp file: {}", temp_path.display()))?;
        file.write_all(&payload)
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
        tokio::task::spawn_blocking(move || persist_temp_path(temp_path, target_path))
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

        self.write_json_atomic(&path, session, &format!("session {}", session.id))
            .await?;

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
        let _guard = self.write_lock.lock().await;
        self.commit_snapshot_under_lock(snapshot).await
    }

    async fn save_snapshot_cas(
        &self,
        snapshot: &SessionSnapshotV1,
        expected_current_digest: Option<&str>,
    ) -> Result<bool> {
        let _guard = self.write_lock.lock().await;
        if let Some(expected) = expected_current_digest {
            let current = match self.load_snapshot_without_wal(&snapshot.session.id).await? {
                Some(existing) => Some(snapshot_content_digest(&existing)?),
                None => None,
            };
            match current.as_deref() {
                Some(digest) if digest == expected => {}
                _ => return Ok(false),
            }
        }
        self.commit_snapshot_under_lock(snapshot).await?;
        Ok(true)
    }

    async fn acquire_writer_lease(&self, holder_id: &str) -> Result<SessionStoreWriterLeaseV1> {
        let _guard = self.write_lock.lock().await;
        self.acquire_writer_lease_under_lock(holder_id).await
    }

    async fn writer_lease(&self) -> Result<Option<SessionStoreWriterLeaseV1>> {
        let _guard = self.write_lock.lock().await;
        self.read_durable_writer_lease_unlocked().await
    }

    async fn watch_commits(&self) -> Result<SessionStoreCommitWatch> {
        Ok(SessionStoreCommitWatch::new(self.commit_watch.subscribe()))
    }

    async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
        self.load_snapshot_without_wal(id).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities {
            atomic_session_snapshots: true,
            append_only_event_log: true,
            aggregate_cas: true,
            lease_fencing: true,
            reference_aware_artifact_gc: true,
            watch: true,
            encrypted_at_rest: self.encryption.is_some(),
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
                match Self::read_session_file_at(&path, self.encryption.as_ref()).await {
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
            let mut retained: Vec<String> = artifacts.retained_uris().into_iter().collect();
            retained.sort();
            snapshot.artifacts = artifacts.artifacts();
            snapshot.retained_artifact_uris = retained;
            return self.save_snapshot(&snapshot).await;
        }

        let artifact_dir = self.artifact_dir(id);
        artifacts.save_to_dir(&artifact_dir).with_context(|| {
            format!(
                "Failed to save artifacts for session {} to {}",
                id,
                artifact_dir.display()
            )
        })?;
        self.seal_artifact_manifest_if_needed(&artifact_dir).await?;
        Ok(())
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

        let bytes = self.load_artifact_manifest_bytes(&artifact_dir).await?;
        let artifacts = ArtifactStore::load_from_manifest_bytes(&bytes).with_context(|| {
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
        self.write_json_atomic(&path, events, &format!("trace events for session {id}"))
            .await
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

        let json = read_json_document(&path, "trace events", self.encryption.as_ref()).await?;
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
        self.write_json_atomic(&path, records, &format!("run records for session {id}"))
            .await
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

        let json = read_json_document(&path, "run records", self.encryption.as_ref()).await?;
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
        self.write_json_atomic(
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

        let json =
            read_json_document(&path, "verification reports", self.encryption.as_ref()).await?;
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
        self.write_json_atomic(&path, tasks, &format!("subagent tasks for session {id}"))
            .await
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
        let json = read_json_document(&path, "subagent tasks", self.encryption.as_ref()).await?;
        let tasks = serde_json::from_slice(&json)
            .with_context(|| format!("Failed to parse subagent tasks from {}", path.display()))?;
        Ok(Some(tasks))
    }

    async fn save_loop_checkpoint(&self, run_id: &str, checkpoint: &LoopCheckpoint) -> Result<()> {
        checkpoint.ensure_addressed_by(run_id)?;
        let path = self.loop_checkpoint_path(run_id);
        self.write_json_atomic(
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
        let checkpoint = Self::read_loop_checkpoint_at(&path, self.encryption.as_ref()).await?;
        checkpoint.ensure_addressed_by(run_id)?;
        Ok(Some(checkpoint))
    }

    async fn delete_loop_checkpoint(&self, run_id: &str) -> Result<()> {
        remove_file_if_exists(&self.loop_checkpoint_path(run_id), "loop checkpoint").await?;
        let legacy = self.legacy_loop_checkpoint_path(run_id);
        if legacy.exists() {
            let checkpoint =
                Self::read_loop_checkpoint_at(&legacy, self.encryption.as_ref()).await?;
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
        self.write_json_atomic(
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
        let checkpoint = Self::read_workflow_checkpoint_at(&path, self.encryption.as_ref()).await?;
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
            let checkpoint =
                Self::read_workflow_checkpoint_at(&legacy, self.encryption.as_ref()).await?;
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
