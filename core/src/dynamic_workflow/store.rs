use super::*;

/// Cross-process serialized adapter for a local Flow event journal.
///
/// `a3s-flow::LocalFileEventStore` deliberately serializes writers only
/// inside one process. Dynamic workflows can be resumed or controlled by a
/// replacement process, so Code adds one small lock-file boundary around the
/// same append-only journal. The adapter does not project or cache workflow
/// state: Flow remains the sole event authority and its optimistic sequence
/// checks still decide whether an append is accepted.
#[derive(Debug, Clone)]
pub struct CrossProcessFlowEventStore {
    pub(super) root: PathBuf,
    pub(super) inner: LocalFileEventStore,
    pub(super) process_lock: Arc<Mutex<()>>,
}

impl CrossProcessFlowEventStore {
    /// Create a cross-process event store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            inner: LocalFileEventStore::new(root.clone()),
            root,
            process_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Return the directory containing the journal files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(super) async fn acquire_file_lock(&self) -> a3s_flow::Result<std::fs::File> {
        tokio::fs::create_dir_all(&self.root).await?;
        let lock_path = self.root.join(".flow-events.lock");
        match tokio::fs::symlink_metadata(&lock_path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(a3s_flow::FlowError::Store(format!(
                    "refusing to use symlinked event-store lock {}",
                    lock_path.display()
                )))
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(a3s_flow::FlowError::Store(format!(
                    "event-store lock {} exists but is not a file",
                    lock_path.display()
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(a3s_flow::FlowError::Io(error)),
        }
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(a3s_flow::FlowError::Io)?;
            file.lock_exclusive().map_err(a3s_flow::FlowError::Io)?;
            Ok(file)
        })
        .await
        .map_err(|error| {
            a3s_flow::FlowError::Store(format!("event-store lock task failed: {error}"))
        })?
    }
}

#[async_trait]
impl FlowEventStore for CrossProcessFlowEventStore {
    async fn append(&self, run_id: &str, event: FlowEvent) -> a3s_flow::Result<FlowEventEnvelope> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.append(run_id, event).await
    }

    async fn append_if_sequence(
        &self,
        run_id: &str,
        expected_sequence: u64,
        event: FlowEvent,
    ) -> a3s_flow::Result<FlowEventEnvelope> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner
            .append_if_sequence(run_id, expected_sequence, event)
            .await
    }

    async fn list(&self, run_id: &str) -> a3s_flow::Result<Vec<FlowEventEnvelope>> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.list(run_id).await
    }

    async fn list_run_ids(&self) -> a3s_flow::Result<Vec<String>> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.list_run_ids().await
    }
}

/// Resolve the durable dynamic workflow history directory for a local workspace.
pub fn dynamic_workflow_store_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root
        .as_ref()
        .join(DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH)
}

/// Recover one completed step output from the exact durable workflow run.
///
/// Recovery is bound to the requested run ID and the original input query. It
/// never acts as a cross-run query cache and never promotes an incomplete step.
pub async fn recover_dynamic_workflow_step_output(
    workspace_root: impl AsRef<Path>,
    run_id: &str,
    expected_query: &str,
    step_id: &str,
) -> Result<Option<Value>> {
    if !safe_workflow_run_id(run_id) || expected_query.is_empty() || step_id.is_empty() {
        return Ok(None);
    }
    let workspace_root = workspace_root.as_ref();
    let store_root = dynamic_workflow_store_path(workspace_root);
    let log_path = store_root.join(format!("{run_id}.jsonl"));
    match tokio::fs::symlink_metadata(&log_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Ok(None)
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("inspect dynamic workflow history {}", log_path.display())
            })
        }
    }
    validate_dynamic_workflow_directory(&workspace_root.join(".a3s"), ".a3s").await?;
    validate_dynamic_workflow_directory(&store_root, ".a3s/workflow").await?;
    validate_dynamic_workflow_log(&log_path).await?;

    let events = LocalFileEventStore::new(store_root).list(run_id).await?;
    let input_matches = events.iter().any(|envelope| {
        matches!(
            &envelope.event,
            FlowEvent::RunCreated { input, .. }
                if input.get("query").and_then(Value::as_str) == Some(expected_query)
        )
    });
    if !input_matches {
        return Ok(None);
    }
    Ok(events
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            FlowEvent::StepCompleted {
                step_id: completed_step_id,
                output,
            } if completed_step_id == step_id => Some(output.clone()),
            _ => None,
        }))
}
