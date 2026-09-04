//! Official `zvec-rust` full-text adapter.
//!
//! This module is intentionally behind the `zvec-rust-fts` feature.  The
//! binding wraps zvec's C API and links a platform dynamic library, so a
//! product release must explicitly package and attest that library before the
//! backend can become the default.  Workspace admission, chunk identity, and
//! source verification remain owned by the surrounding Code catalog.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use tempfile::TempDir;
use zvec_rust::{
    Collection, CollectionSchema, DataType, Doc, FieldSchema, Fts, IndexParams, SearchQuery,
};

const INSERT_BATCH_SIZE: usize = 256;
const ESTIMATED_DOCUMENT_OVERHEAD: usize = 64;
// A native collection owns multiple RocksDB descriptors. Keep a small global
// read-only cache for hot partitions, but leave enough headroom for the host
// process and for transient opens. Partitions beyond this cap use the same
// close-after-query path as minimal-resource environments.
const MAX_OPEN_COLLECTIONS: usize = 4;
// A zvec close releases the collection lock synchronously, but its native
// segment teardown can briefly outlive the FFI call under heavy parallel
// churn. Keep retries bounded while allowing that teardown to settle.
const OPEN_RETRY_DELAYS_MS: &[u64] = &[1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024];

/// Process-wide zvec initialization result.
///
/// zvec's C API is process-global.  We initialize it once and deliberately do
/// not call `shutdown` from a session or collection destructor: another
/// session may still own a collection handle, and shutting down underneath it
/// would make safe Rust handles unsound.  Process teardown reclaims the native
/// library resources.
static INITIALIZATION: OnceLock<Result<(), String>> = OnceLock::new();

/// Bound native collection lifetimes across all workspace partitions.
///
/// zvec collections are backed by RocksDB. Even read-only opens allocate
/// several descriptors, and a catalog can build or query many file
/// partitions concurrently. The surrounding catalog remains immutable, but
/// native handles are deliberately admitted one at a time so a burst of file
/// updates cannot exhaust the process descriptor budget.
static NATIVE_OPERATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

static OPEN_COLLECTIONS: AtomicUsize = AtomicUsize::new(0);

/// zvec-rust 0.7.0 opens native files without O_CLOEXEC. Marking descriptors
/// after an operation starts is not sufficient when another thread forks in
/// that window: the child can inherit a RocksDB LOCK descriptor before the
/// sweep runs. A process-wide pthread gate closes that race. Its atfork
/// prepare callback waits for every guarded zvec operation to finish before
/// fork, while the parent/child callbacks release the gate again.
#[cfg(unix)]
struct NativeForkGate {
    mutex: libc::pthread_mutex_t,
}

#[cfg(unix)]
// The pthread mutex is initialized once and then accessed only through the C
// lock/unlock operations below. It intentionally lives until process exit so
// an atfork callback never observes a dangling pointer.
unsafe impl Sync for NativeForkGate {}

#[cfg(unix)]
impl NativeForkGate {
    fn new() -> Self {
        Self {
            mutex: libc::PTHREAD_MUTEX_INITIALIZER,
        }
    }

    fn as_ptr(&self) -> *mut libc::pthread_mutex_t {
        // The mutex is mutated by pthreads, not by Rust, and remains allocated
        // for the lifetime of the process.
        (&self.mutex as *const libc::pthread_mutex_t).cast_mut()
    }
}

#[cfg(unix)]
static NATIVE_FORK_GATE: OnceLock<NativeForkGate> = OnceLock::new();

#[cfg(unix)]
unsafe extern "C" fn native_atfork_prepare() {
    if let Some(gate) = NATIVE_FORK_GATE.get() {
        // pthread_atfork has no useful recovery path if locking fails. The
        // mutex is process-private and initialized once, so a non-zero result
        // would indicate an unrecoverable runtime failure; leaving the fork
        // blocked is safer than allowing a child to inherit zvec descriptors.
        let _ = libc::pthread_mutex_lock(gate.as_ptr());
    }
}

#[cfg(unix)]
unsafe extern "C" fn native_atfork_parent() {
    if let Some(gate) = NATIVE_FORK_GATE.get() {
        let _ = libc::pthread_mutex_unlock(gate.as_ptr());
    }
}

#[cfg(unix)]
unsafe extern "C" fn native_atfork_child() {
    if let Some(gate) = NATIVE_FORK_GATE.get() {
        let _ = libc::pthread_mutex_unlock(gate.as_ptr());
    }
}

#[cfg(unix)]
fn install_native_atfork_gate() -> Result<(), String> {
    static REGISTRATION: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTRATION
        .get_or_init(|| {
            NATIVE_FORK_GATE.get_or_init(NativeForkGate::new);
            let result = unsafe {
                libc::pthread_atfork(
                    Some(native_atfork_prepare),
                    Some(native_atfork_parent),
                    Some(native_atfork_child),
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(format!("failed to register zvec atfork gate: {result}"))
            }
        })
        .clone()
}

#[cfg(not(unix))]
fn install_native_atfork_gate() -> Result<(), String> {
    Ok(())
}

struct NativeOperationGuard {
    _serial: MutexGuard<'static, ()>,
}

struct NativeBoundaryGuard {
    resource: Option<super::NativeResourceOperationGuard>,
    #[cfg(unix)]
    fork_gate: &'static NativeForkGate,
}

impl Drop for NativeBoundaryGuard {
    fn drop(&mut self) {
        // Release the Rust resource gate while the pthread gate is still held.
        // Otherwise a direct fork (which relies on pthread_atfork rather than
        // Code's process helper) could slip into the tiny field-drop window and
        // inherit a descriptor that is still owned by zvec.
        self.resource.take();
        #[cfg(unix)]
        {
            let result = unsafe { libc::pthread_mutex_unlock(self.fork_gate.as_ptr()) };
            debug_assert_eq!(result, 0);
        }
    }
}

fn native_operation_lock() -> Result<NativeOperationGuard, String> {
    install_native_atfork_gate()?;
    let serial = NATIVE_OPERATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "zvec native operation lock poisoned".to_owned())?;
    Ok(NativeOperationGuard { _serial: serial })
}

fn native_boundary() -> Result<NativeBoundaryGuard, String> {
    install_native_atfork_gate()?;
    let resource = super::native_resource_operation()?;
    #[cfg(unix)]
    {
        let Some(fork_gate) = NATIVE_FORK_GATE.get() else {
            drop(resource);
            return Err("zvec atfork gate was not initialized".to_owned());
        };
        let result = unsafe { libc::pthread_mutex_lock(fork_gate.as_ptr()) };
        if result != 0 {
            drop(resource);
            return Err(format!("failed to lock zvec atfork gate: {result}"));
        }
        Ok(NativeBoundaryGuard {
            resource: Some(resource),
            fork_gate,
        })
    }
    #[cfg(not(unix))]
    {
        Ok(NativeBoundaryGuard {
            resource: Some(resource),
        })
    }
}

fn ensure_initialized() -> Result<(), String> {
    // Initialization touches the process-global C API just like collection
    // operations do. Keep it inside the same resource/fork boundary so a
    // host process spawn cannot observe a half-initialized native runtime.
    let _boundary = native_boundary()?;
    INITIALIZATION
        .get_or_init(|| {
            if zvec_rust::is_initialized() {
                return Ok(());
            }
            match zvec_rust::initialize(None) {
                Ok(()) => Ok(()),
                Err(_error) if zvec_rust::is_initialized() => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        })
        .clone()
}

/// A bounded, temporary zvec FTS collection plus the caller's ordinal map.
pub(crate) struct ZvecRustLexicalIndex {
    // Keep the collection closed between operations. zvec's FTS index is
    // backed by RocksDB and one open handle consumes several descriptors. A
    // workspace can contain hundreds of partitions, so reopening a bounded
    // snapshot is safer than retaining one native handle per file. The
    // process-wide gate and bounded retry below keep open/close churn safe.
    // Hot partitions may populate this slot lazily; the global counter keeps
    // the aggregate descriptor footprint bounded across sessions.
    collection: Mutex<Option<Collection>>,
    collection_path: PathBuf,
    // The directory owns the persisted temporary collection and is removed
    // only after the path has no open native handles.
    _temp_dir: TempDir,
    terms: HashSet<String>,
    /// zvec primary keys are deliberately generated from the dense ordinal:
    /// the C API accepts a narrower character set than Code chunk ids (which
    /// include separators and a digest prefix).
    native_ordinals: HashMap<String, usize>,
    document_count: usize,
    estimated_bytes: usize,
}

impl ZvecRustLexicalIndex {
    pub(crate) fn build<I, K, T>(documents: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str>,
        T: AsRef<str>,
    {
        let mut seen_keys = HashSet::new();
        let mut prepared = Vec::<(String, String)>::new();
        let mut terms = HashSet::new();

        for (key, text) in documents {
            let key = key.as_ref();
            if key.is_empty() || key.contains('\0') {
                return Err(
                    "lexical document key must be non-empty and contain no NUL byte".into(),
                );
            }
            if !seen_keys.insert(key.to_owned()) {
                return Err("lexical document keys must be unique".into());
            }

            // Code's tokenizer is the canonical workspace analyzer.  Feeding
            // normalized tokens to zvec keeps identifier and CJK behavior
            // stable while delegating postings/BM25 scoring to zvec.
            let tokens = super::lexical::tokenize(text.as_ref());
            if tokens.is_empty() {
                continue;
            }
            terms.extend(tokens.iter().cloned());
            prepared.push((key.to_owned(), tokens.join(" ")));
        }

        ensure_initialized()?;

        let mut body = FieldSchema::new("body", DataType::String, false, 0)
            .map_err(|error| error.to_string())?;
        let fts =
            IndexParams::fts(Some("whitespace"), None, None).map_err(|error| error.to_string())?;
        body.set_index_params(&fts)
            .map_err(|error| error.to_string())?;
        let schema = CollectionSchema::builder("workspace_lexical")
            .add_field(body)
            .build()
            .map_err(|error| error.to_string())?;

        let temp_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let collection_path = temp_dir
            .path()
            .join("collection")
            .to_str()
            .ok_or_else(|| "zvec lexical path is not UTF-8".to_owned())?
            .to_owned();
        let native_guard = native_operation_lock()?;
        let collection = {
            let _boundary = native_boundary()?;
            let collection = Collection::create_and_open(&collection_path, &schema, None)
                .map_err(|error| error.to_string())?;
            mark_collection_fds_close_on_exec(std::path::Path::new(&collection_path))?;
            collection
        };

        let mut native_ordinals = HashMap::with_capacity(prepared.len());
        let mut next_ordinal = 0usize;
        for batch in prepared.chunks(INSERT_BATCH_SIZE) {
            let mut docs = Vec::with_capacity(batch.len());
            for (_key, text) in batch {
                let ordinal = next_ordinal;
                next_ordinal = next_ordinal.saturating_add(1);
                let native_key = format!("d{ordinal}");
                native_ordinals.insert(native_key.clone(), ordinal);
                let mut document = Doc::new().map_err(|error| error.to_string())?;
                // The generated key is ASCII and NUL-free, so zvec's
                // infallible setter cannot panic for an admitted document.
                document.set_pk(&native_key);
                document
                    .add_string("body", text)
                    .map_err(|error| error.to_string())?;
                docs.push(document);
            }
            let references = docs.iter().collect::<Vec<_>>();
            let result = {
                let _boundary = native_boundary()?;
                let result = collection.insert(&references);
                // The C API may retain document-backed buffers until the
                // insert call returns. Destroy both the borrowed pointer list
                // and owning documents before leaving the guarded boundary.
                drop(references);
                drop(docs);
                mark_collection_fds_close_on_exec(std::path::Path::new(&collection_path))?;
                result.map_err(|error| error.to_string())?
            };
            if result.error_count != 0 {
                return Err(format!(
                    "zvec lexical insert rejected {} document(s)",
                    result.error_count
                ));
            }
        }

        // Flush makes the persistence contract observable in the qualification
        // tests and keeps the temporary collection's index state complete.
        {
            let _boundary = native_boundary()?;
            collection.flush().map_err(|error| error.to_string())?;
            mark_collection_fds_close_on_exec(std::path::Path::new(&collection_path))?;
            // Close before returning. Queries reopen the immutable snapshot
            // for the shortest possible scope, keeping descriptor usage
            // bounded by one active native operation.
            collection.close().map_err(|error| error.to_string())?;
            wait_for_collection_lock_release(std::path::Path::new(&collection_path))?;
            // Schema/query FFI objects are process-global handles as well;
            // release them while the native boundary is still held.
            drop(schema);
            drop(fts);
        }
        drop(native_guard);

        // zvec's native FTS/RocksDB files have a substantially larger fixed
        // footprint than the logical token stream. Account for the actual
        // persisted bytes so the surrounding catalog budget remains a real
        // safety bound instead of an optimistic in-memory estimate.
        let estimated_bytes = directory_size(temp_dir.path())?.max(prepared.iter().fold(
            0usize,
            |total, (key, text)| {
                total
                    .saturating_add(key.len())
                    .saturating_add(text.len())
                    .saturating_add(ESTIMATED_DOCUMENT_OVERHEAD)
            },
        ));

        Ok(Self {
            collection: Mutex::new(None),
            collection_path: PathBuf::from(collection_path),
            _temp_dir: temp_dir,
            terms,
            document_count: prepared.len(),
            native_ordinals,
            estimated_bytes,
        })
    }

    pub(crate) fn document_count(&self) -> usize {
        self.document_count
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub(crate) fn has_any_term(&self, terms: &[String]) -> bool {
        terms.iter().any(|term| self.terms.contains(term))
    }

    pub(crate) fn search(
        &self,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<(usize, f64)>, String> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut fts = Fts::new().map_err(|error| error.to_string())?;
        fts.set_match_string(&terms.join(" "))
            .map_err(|error| error.to_string())?;
        let topk =
            i32::try_from(limit).map_err(|_| "lexical result limit exceeds i32".to_owned())?;
        let mut query = SearchQuery::fts("body", &fts, topk).map_err(|error| error.to_string())?;
        query
            .set_output_fields(&[])
            .map_err(|error| error.to_string())?;
        let mut options = zvec_rust::CollectionOptions::new().map_err(|error| error.to_string())?;
        options
            .set_read_only(true)
            .map_err(|error| error.to_string())?;
        let native_guard = native_operation_lock()?;
        let collection_path = self
            .collection_path
            .to_str()
            .ok_or_else(|| "zvec lexical path is not UTF-8".to_owned())?;
        let mut cached_slot = Some(
            self.collection
                .lock()
                .map_err(|_| "zvec lexical collection lock poisoned".to_owned())?,
        );
        let mut cached = cached_slot.as_ref().is_some_and(|slot| slot.is_some());
        if !cached && reserve_open_collection() {
            match open_read_only_with_retry(collection_path, &options) {
                Ok(collection) => {
                    **cached_slot
                        .as_mut()
                        .ok_or_else(|| "zvec lexical collection guard missing".to_owned())? =
                        Some(collection);
                    cached = true;
                }
                Err(error) => {
                    release_open_collection();
                    drop(cached_slot.take());
                    drop(native_guard);
                    return Err(error);
                }
            }
        }

        let (documents, transient_collection) = if cached {
            let collection = cached_slot
                .as_ref()
                .and_then(|slot| slot.as_ref())
                .ok_or_else(|| "zvec lexical collection cache is empty".to_owned())?;
            (query_collection(collection, &query, collection_path)?, None)
        } else {
            // The bounded cache is full. Open this cold partition only for the
            // current query, then close it before releasing the native gate.
            drop(cached_slot.take());
            let collection = open_read_only_with_retry(collection_path, &options)?;
            let documents = query_collection(&collection, &query, collection_path)?;
            (documents, Some(collection))
        };

        let hits = map_query_documents(&self.native_ordinals, &documents)?;
        // Query documents are independent FFI objects, but destroy them before
        // closing a transient collection so no native result snapshot can keep
        // a segment resource alive across the lock boundary.
        drop(documents);
        if let Some(collection) = transient_collection {
            let _boundary = native_boundary()?;
            drop(collection);
            wait_for_collection_lock_release(std::path::Path::new(collection_path))?;
        }
        drop(cached_slot);
        {
            let _boundary = native_boundary()?;
            drop(options);
            drop(query);
            drop(fts);
        }
        drop(native_guard);
        Ok(hits)
    }
}

impl Drop for ZvecRustLexicalIndex {
    fn drop(&mut self) {
        // `get_mut` avoids blocking during normal teardown. If a query thread
        // poisoned the mutex, recover the slot and still close the native
        // handle; leaking it would keep the temporary directory locked.
        let collection = match self.collection.get_mut() {
            Ok(slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(collection) = collection else {
            return;
        };
        release_open_collection();

        // Collection::Drop invokes the C close API. Keep it inside the same
        // native/fork gates as every other zvec boundary and before TempDir's
        // destructor removes the files.
        if let Ok(_serial) = native_operation_lock() {
            if let Ok(_boundary) = native_boundary() {
                drop(collection);
                return;
            }
        }
        drop(collection);
    }
}

fn reserve_open_collection() -> bool {
    OPEN_COLLECTIONS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_OPEN_COLLECTIONS).then_some(count + 1)
        })
        .is_ok()
}

fn release_open_collection() {
    let previous = OPEN_COLLECTIONS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0);
}

fn query_collection(
    collection: &Collection,
    query: &SearchQuery,
    collection_path: &str,
) -> Result<Vec<zvec_rust::Doc>, String> {
    let _boundary = native_boundary()?;
    let documents = collection.query(query).map_err(|error| error.to_string())?;
    mark_collection_fds_close_on_exec(std::path::Path::new(collection_path))?;
    Ok(documents)
}

fn map_query_documents(
    native_ordinals: &HashMap<String, usize>,
    documents: &[zvec_rust::Doc],
) -> Result<Vec<(usize, f64)>, String> {
    let mut hits = Vec::with_capacity(documents.len());
    for document in documents {
        let key = document
            .get_pk()
            .ok_or_else(|| "zvec lexical result omitted its primary key".to_owned())?;
        let ordinal = native_ordinals
            .get(key)
            .copied()
            .ok_or_else(|| "zvec lexical result returned an unknown primary key".to_owned())?;
        let score = f64::from(document.get_score());
        if score.is_finite() {
            hits.push((ordinal, score));
        }
    }
    Ok(hits)
}

/// Mark every descriptor currently opened below a native collection directory
/// as close-on-exec. zvec-rust 0.7.0 delegates file creation to zvec's C++
/// `ailego::File`, which does not set `FD_CLOEXEC` on Unix. Without this
/// boundary, a shell/LSP child spawned while a query is running can inherit
/// `collection/LOCK` and keep it held after the parent collection is dropped.
///
/// The sweep is intentionally best-effort for platforms without a proc-fd
/// view. Supported release targets (macOS and Linux) expose `/dev/fd` or
/// `/proc/self/fd`; Windows handles created by zvec are non-inheritable by
/// default and use the no-op branch.
fn mark_collection_fds_close_on_exec(collection_root: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let expected =
            fs::canonicalize(collection_root).unwrap_or_else(|_| collection_root.to_path_buf());
        let mut proc_dir = None;
        for candidate in [
            std::path::Path::new("/proc/self/fd"),
            std::path::Path::new("/dev/fd"),
        ] {
            if candidate.is_dir() {
                proc_dir = Some(candidate);
                break;
            }
        }
        let Some(proc_dir) = proc_dir else {
            return Ok(());
        };

        for entry in fs::read_dir(proc_dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let Some(fd) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let Some(target_path) = fd_path(fd, &entry.path()) else {
                continue;
            };
            let target_path =
                fs::canonicalize(&target_path).unwrap_or_else(|_| target_path.to_path_buf());
            if target_path != expected && !target_path.starts_with(&expected) {
                continue;
            }
            // A descriptor can disappear between readlink and fcntl when a
            // native teardown races this hygiene sweep; that is harmless.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 {
                continue;
            }
            let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EBADF) {
                    return Err(format!(
                        "failed to mark zvec descriptor {fd} close-on-exec: {error}"
                    ));
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = collection_root;
    }
    Ok(())
}

#[cfg(unix)]
fn fd_path(fd: i32, _descriptor_path: &std::path::Path) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0i8; libc::PATH_MAX as usize];
        let result = unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) };
        if result < 0 {
            return None;
        }
        let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        Some(std::path::PathBuf::from(
            path.to_string_lossy().into_owned(),
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let target = fs::read_link(_descriptor_path).ok()?;
        Some(std::path::PathBuf::from(
            target
                .to_string_lossy()
                .trim_end_matches(" (deleted)")
                .to_owned(),
        ))
    }
}

/// zvec's collection lock is deliberately non-blocking. Retry only the
/// specific lock contention error; all schema, corruption, and permission
/// errors remain fail-closed.
fn open_read_only_with_retry(
    path: &str,
    options: &zvec_rust::CollectionOptions,
) -> Result<Collection, String> {
    for (attempt, delay_ms) in std::iter::once(0)
        .chain(OPEN_RETRY_DELAYS_MS.iter().copied())
        .enumerate()
    {
        if delay_ms != 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        let result = {
            let _boundary = native_boundary()?;
            match Collection::open(path, Some(options)) {
                Ok(collection) => {
                    mark_collection_fds_close_on_exec(std::path::Path::new(path))?;
                    Ok(collection)
                }
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(collection) => {
                return Ok(collection);
            }
            Err(error) => {
                let message = error.to_string();
                let transient_lock = message.contains("Can't lock read-only collection")
                    || message.contains("Can't lock read-write collection");
                if !transient_lock || attempt == OPEN_RETRY_DELAYS_MS.len() {
                    return Err(message);
                }
            }
        }
    }
    Err("zvec collection open retry loop exhausted".to_owned())
}

/// Wait until the native collection's advisory lock is fully released after
/// dropping a zvec handle. zvec's C++ teardown can release its internal
/// segments just after the FFI close call returns; probing the actual LOCK
/// inode keeps the next open from racing that teardown. This is intentionally
/// bounded and only treats EWOULDBLOCK/EAGAIN as transient.
fn wait_for_collection_lock_release(collection_root: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let lock_path = collection_root.join("LOCK");
        for (attempt, delay_ms) in std::iter::once(0)
            .chain(OPEN_RETRY_DELAYS_MS.iter().copied())
            .enumerate()
        {
            if delay_ms != 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            let file = match fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(file) => file,
                Err(error) => {
                    if attempt == OPEN_RETRY_DELAYS_MS.len() {
                        return Err(format!(
                            "failed to probe zvec collection lock {}: {error}",
                            lock_path.display()
                        ));
                    }
                    continue;
                }
            };
            let fd = file.as_raw_fd();
            let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                let _ = unsafe { libc::flock(fd, libc::LOCK_UN) };
                drop(file);
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            let transient = matches!(
                error.raw_os_error(),
                Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
            );
            drop(file);
            if !transient || attempt == OPEN_RETRY_DELAYS_MS.len() {
                return Err(format!(
                    "zvec collection lock did not settle at {}: {error}",
                    lock_path.display()
                ));
            }
        }
        Err("zvec collection lock probe loop exhausted".to_owned())
    }
    #[cfg(not(unix))]
    {
        let _ = collection_root;
        Ok(())
    }
}

fn directory_size(root: &std::path::Path) -> Result<usize, String> {
    fn visit(path: &std::path::Path) -> Result<usize, String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "zvec lexical collection contains an unexpected symlink: {}",
                path.display()
            ));
        }
        if metadata.is_file() {
            return usize::try_from(metadata.len())
                .map_err(|_| "zvec lexical file size exceeds usize".to_owned());
        }
        if !metadata.is_dir() {
            return Ok(0);
        }
        let mut total = 0usize;
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            total = total
                .checked_add(visit(&entry.path())?)
                .ok_or_else(|| "zvec lexical directory size overflows usize".to_owned())?;
        }
        Ok(total)
    }

    visit(root)
}

#[cfg(test)]
mod tests {
    use super::{open_read_only_with_retry, ZvecRustLexicalIndex};
    use zvec_rust::Collection;

    #[test]
    fn rejects_invalid_or_duplicate_document_keys_before_native_initialization() {
        assert!(matches!(
            ZvecRustLexicalIndex::build([("", "text")]),
            Err(error) if error.contains("non-empty")
        ));
        assert!(matches!(
            ZvecRustLexicalIndex::build([("bad\0key", "text")]),
            Err(error) if error.contains("NUL")
        ));
        assert!(matches!(
            ZvecRustLexicalIndex::build([("same", "first"), ("same", "second")]),
            Err(error) if error.contains("unique")
        ));
    }

    #[test]
    fn builds_and_queries_a_multi_document_native_fts_partition() {
        // Two documents intentionally cross the lexical adapter's tiny
        // partition fast-path threshold. This test proves that the actual
        // zvec-rust collection is opened, flushed, reopened, and mapped back
        // to caller ordinals rather than only exercising the portable scorer.
        let index = ZvecRustLexicalIndex::build([
            ("first", "cache invalidation policy"),
            ("second", "cache expiry policy"),
        ])
        .expect("native zvec FTS partition must build");
        let terms = ["cache".to_owned(), "invalidation".to_owned()];
        let hits = index
            .search(&terms, 2)
            .expect("native zvec FTS query must work");
        assert_eq!(hits.first().map(|hit| hit.0), Some(0));
        assert!(hits
            .iter()
            .all(|(_, score)| score.is_finite() && *score > 0.0));
        assert_eq!(index.document_count(), 2);
        assert!(index.estimated_bytes() > 0);
    }

    #[test]
    fn builds_and_queries_a_single_document_native_fts_partition() {
        let index = ZvecRustLexicalIndex::build([("only", "single document cache policy")])
            .expect("single-document native zvec FTS partition must build");
        let hits = index
            .search(&["cache".to_owned()], 1)
            .expect("single-document native zvec FTS query must work");
        assert_eq!(hits.first().map(|hit| hit.0), Some(0));
        assert_eq!(index.document_count(), 1);
    }

    #[test]
    fn native_ordinal_mapping_stays_dense_when_empty_documents_are_skipped() {
        let index = ZvecRustLexicalIndex::build([
            ("empty", "   "),
            ("needle", "needle appears here"),
            ("other", "unrelated content"),
        ])
        .expect("native zvec FTS partition must build");
        let hits = index
            .search(&["needle".to_owned()], 2)
            .expect("native zvec FTS query must work");
        assert_eq!(hits.first().map(|hit| hit.0), Some(0));
        assert_eq!(index.document_count(), 2);
    }

    #[test]
    fn native_collections_survive_parallel_build_and_query_churn() {
        let workers = (0..32)
            .map(|worker| {
                std::thread::spawn(move || {
                    let index = ZvecRustLexicalIndex::build([
                        ("first", format!("cache invalidation policy {worker}")),
                        ("second", format!("cache expiry policy {worker}")),
                    ])
                    .expect("native zvec FTS partition must build under contention");
                    let hits = index
                        .search(&["cache".to_owned(), "invalidation".to_owned()], 2)
                        .expect("native zvec FTS query must work under contention");
                    assert_eq!(hits.first().map(|hit| hit.0), Some(0));
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("native worker must not panic");
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_collection_lock_is_not_inherited_by_child_process() {
        let index = ZvecRustLexicalIndex::build([
            ("first", "cache invalidation policy"),
            ("second", "cache expiry policy"),
        ])
        .unwrap();
        let path = index.collection_path.to_str().unwrap();
        let mut options = zvec_rust::CollectionOptions::new().unwrap();
        options.set_read_only(true).unwrap();
        let holder = open_read_only_with_retry(path, &options).unwrap();
        let mut command = std::process::Command::new("sleep");
        command.arg("2");
        let mut child = crate::tools::process::spawn_std_with_native_gate(&mut command)
            .expect("sleep must start");
        drop(holder);

        let mut writable = zvec_rust::CollectionOptions::new().unwrap();
        writable.set_read_only(false).unwrap();
        let reopened = Collection::open(path, Some(&writable))
            .expect("a child must not retain the zvec collection lock");
        drop(reopened);
        let _ = child.kill();
        let _ = child.wait();
    }
}
