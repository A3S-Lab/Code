use std::{
    error::Error,
    sync::atomic::{AtomicUsize, Ordering},
};

use tempfile::TempDir;
use tokio::sync::Barrier;

use super::*;

#[derive(Debug)]
struct MockRuntime {
    id: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestError(&'static str);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for TestError {}

fn registry(
    config: RegistryConfig,
    shutdowns: Arc<Mutex<Vec<usize>>>,
) -> LocalCodeIntelligenceRegistry<MockRuntime, TestError> {
    LocalCodeIntelligenceRegistry::new(config, move |runtime: Arc<MockRuntime>| {
        let shutdowns = Arc::clone(&shutdowns);
        async move {
            mutex_lock(&shutdowns).push(runtime.id);
            Ok(())
        }
    })
}

async fn registry_key(scope: &str, root: &Path, layout_hash: u64) -> RegistryKey {
    RegistryKey::new(scope, root, layout_hash).await.unwrap()
}

#[tokio::test]
async fn key_canonicalizes_equivalent_roots() {
    let workspace = TempDir::new().unwrap();
    let nested = workspace.path().join("nested");
    tokio::fs::create_dir(&nested).await.unwrap();
    let aliased = nested.join("..").join("nested");

    let direct = registry_key("tenant-a", &nested, 42).await;
    let normalized = registry_key("tenant-a", &aliased, 42).await;

    assert_eq!(direct, normalized);
    assert_eq!(
        direct.canonical_root(),
        tokio::fs::canonicalize(&nested).await.unwrap()
    );
    assert_eq!(direct.isolation_scope(), "tenant-a");
    assert_eq!(direct.layout_hash(), 42);
}

#[tokio::test]
async fn concurrent_acquire_starts_exactly_once() {
    let workspace = TempDir::new().unwrap();
    let key = registry_key("tenant-a", workspace.path(), 1).await;
    let starts = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Barrier::new(2));
    let registry = registry(RegistryConfig::default(), Arc::new(Mutex::new(Vec::new())));

    let first = {
        let registry = registry.clone();
        let key = key.clone();
        let starts = Arc::clone(&starts);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            registry
                .acquire(key, move |_| async move {
                    starts.fetch_add(1, Ordering::SeqCst);
                    release.wait().await;
                    Ok(MockRuntime { id: 7 })
                })
                .await
                .unwrap()
        })
    };
    while starts.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let mut tasks = Vec::new();
    for _ in 0..15 {
        let registry = registry.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            registry
                .acquire(key, |_| async {
                    panic!("cancelled caller's owned factory must remain single-flight");
                    #[allow(unreachable_code)]
                    Ok(MockRuntime { id: 99 })
                })
                .await
                .unwrap()
        }));
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    release.wait().await;

    let mut leases = Vec::new();
    for task in tasks {
        let lease = task.await.unwrap();
        assert_eq!(lease.id, 7);
        assert_eq!(lease.key(), &key);
        leases.push(lease);
    }
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(registry.entry_count(), 1);
    drop(leases);
}

#[tokio::test]
async fn failed_factory_is_shared_then_next_acquire_retries() {
    let workspace = TempDir::new().unwrap();
    let key = registry_key("tenant-a", workspace.path(), 1).await;
    let starts = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Barrier::new(2));
    let registry = registry(RegistryConfig::default(), Arc::new(Mutex::new(Vec::new())));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let registry = registry.clone();
        let key = key.clone();
        let starts = Arc::clone(&starts);
        let release = Arc::clone(&release);
        tasks.push(tokio::spawn(async move {
            registry
                .acquire(key, move |_| async move {
                    starts.fetch_add(1, Ordering::SeqCst);
                    release.wait().await;
                    Err(TestError("start failed"))
                })
                .await
        }));
    }
    while starts.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    loop {
        let all_waiters_attached = {
            let state = mutex_lock(&registry.inner.state);
            state
                .entries
                .get(&key)
                .is_some_and(|entry| Arc::strong_count(entry) >= 4)
        };
        if all_waiters_attached {
            break;
        }
        tokio::task::yield_now().await;
    }
    release.wait().await;

    let first = tasks.remove(0).await.unwrap().unwrap_err();
    let second = tasks.remove(0).await.unwrap().unwrap_err();
    let (RegistryAcquireError::Factory(first), RegistryAcquireError::Factory(second)) =
        (first, second)
    else {
        panic!("expected shared factory errors");
    };
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(registry.entry_count(), 0);

    let starts_for_retry = Arc::clone(&starts);
    let lease = registry
        .acquire(key, move |_| async move {
            starts_for_retry.fetch_add(1, Ordering::SeqCst);
            Ok(MockRuntime { id: 8 })
        })
        .await
        .unwrap();
    assert_eq!(lease.id, 8);
    assert_eq!(starts.load(Ordering::SeqCst), 2);

    let panic_key = registry_key("tenant-a", workspace.path(), 2).await;
    let panic = registry
        .acquire(panic_key.clone(), |_| async {
            panic!("factory panic");
            #[allow(unreachable_code)]
            Ok(MockRuntime { id: 9 })
        })
        .await
        .unwrap_err();
    assert!(matches!(
        panic,
        RegistryAcquireError::FactoryPanicked { .. }
    ));
    let recovered = registry
        .acquire(panic_key, |_| async { Ok(MockRuntime { id: 9 }) })
        .await
        .unwrap();
    assert_eq!(recovered.id, 9);
}

#[tokio::test]
async fn tenant_root_and_layout_are_independent_isolation_dimensions() {
    let first_root = TempDir::new().unwrap();
    let second_root = TempDir::new().unwrap();
    let keys = vec![
        registry_key("tenant-a", first_root.path(), 1).await,
        registry_key("tenant-b", first_root.path(), 1).await,
        registry_key("tenant-a", second_root.path(), 1).await,
        registry_key("tenant-a", first_root.path(), 2).await,
    ];
    let starts = Arc::new(AtomicUsize::new(0));
    let registry = registry(RegistryConfig::default(), Arc::new(Mutex::new(Vec::new())));
    let mut leases = Vec::new();

    for key in &keys {
        let starts = Arc::clone(&starts);
        leases.push(
            registry
                .acquire(key.clone(), move |_| async move {
                    let id = starts.fetch_add(1, Ordering::SeqCst) + 1;
                    Ok(MockRuntime { id })
                })
                .await
                .unwrap(),
        );
    }
    assert_eq!(starts.load(Ordering::SeqCst), 4);
    assert_eq!(registry.entry_count(), 4);

    let reused = registry
        .acquire(keys[0].clone(), |_| async move {
            panic!("factory must not run for an existing key");
            #[allow(unreachable_code)]
            Ok(MockRuntime { id: 99 })
        })
        .await
        .unwrap();
    assert_eq!(reused.id, leases[0].id);
    assert_eq!(starts.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn active_lease_blocks_cleanup_and_release_allows_ttl_reclaim() {
    let workspace = TempDir::new().unwrap();
    let key = registry_key("tenant-a", workspace.path(), 1).await;
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let registry = registry(
        RegistryConfig::new(Duration::ZERO, 8),
        Arc::clone(&shutdowns),
    );
    let lease = registry
        .acquire(key.clone(), |_| async { Ok(MockRuntime { id: 1 }) })
        .await
        .unwrap();

    let active = registry.cleanup_idle().await;
    assert!(active.removed.is_empty());
    assert!(active.errors.is_empty());
    assert_eq!(registry.entry_count(), 1);

    drop(lease);
    let idle = registry.cleanup_idle().await;
    assert_eq!(idle.removed, vec![key]);
    assert!(idle.errors.is_empty());
    assert_eq!(*mutex_lock(&shutdowns), vec![1]);
    assert_eq!(registry.entry_count(), 0);
}

#[tokio::test]
async fn cleanup_enforces_idle_lru_bound() {
    let first_root = TempDir::new().unwrap();
    let second_root = TempDir::new().unwrap();
    let first_key = registry_key("tenant-a", first_root.path(), 1).await;
    let second_key = registry_key("tenant-a", second_root.path(), 1).await;
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let registry = registry(
        RegistryConfig::new(Duration::from_secs(60 * 60), 1),
        Arc::clone(&shutdowns),
    );

    let first = registry
        .acquire(first_key.clone(), |_| async { Ok(MockRuntime { id: 1 }) })
        .await
        .unwrap();
    drop(first);
    let second = registry
        .acquire(second_key.clone(), |_| async { Ok(MockRuntime { id: 2 }) })
        .await
        .unwrap();
    drop(second);

    let report = registry.cleanup_idle().await;
    assert_eq!(report.removed, vec![first_key]);
    assert!(report.errors.is_empty());
    assert_eq!(*mutex_lock(&shutdowns), vec![1]);
    assert_eq!(registry.entry_count(), 1);

    let reused = registry
        .acquire(second_key, |_| async {
            panic!("most recently used idle runtime should remain cached");
            #[allow(unreachable_code)]
            Ok(MockRuntime { id: 99 })
        })
        .await
        .unwrap();
    assert_eq!(reused.id, 2);
}

#[tokio::test]
async fn shutdown_all_is_terminal_and_includes_active_runtimes() {
    let first_root = TempDir::new().unwrap();
    let second_root = TempDir::new().unwrap();
    let first_key = registry_key("tenant-a", first_root.path(), 1).await;
    let second_key = registry_key("tenant-a", second_root.path(), 1).await;
    let shutdowns = Arc::new(Mutex::new(Vec::new()));
    let registry = registry(RegistryConfig::default(), Arc::clone(&shutdowns));
    let active = registry
        .acquire(first_key.clone(), |_| async { Ok(MockRuntime { id: 1 }) })
        .await
        .unwrap();
    let idle = registry
        .acquire(second_key.clone(), |_| async { Ok(MockRuntime { id: 2 }) })
        .await
        .unwrap();
    drop(idle);

    let mut report = registry.shutdown_all().await;
    report
        .removed
        .sort_by(|left, right| left.canonical_root().cmp(right.canonical_root()));
    let mut expected = vec![first_key.clone(), second_key];
    expected.sort_by(|left, right| left.canonical_root().cmp(right.canonical_root()));
    assert_eq!(report.removed, expected);
    assert!(report.errors.is_empty());
    assert!(!registry.is_accepting());
    assert_eq!(registry.entry_count(), 0);
    let mut stopped = mutex_lock(&shutdowns).clone();
    stopped.sort_unstable();
    assert_eq!(stopped, vec![1, 2]);
    assert_eq!(active.id, 1);

    let starts = Arc::new(AtomicUsize::new(0));
    let starts_for_factory = Arc::clone(&starts);
    let result = registry
        .acquire(first_key, move |_| async move {
            starts_for_factory.fetch_add(1, Ordering::SeqCst);
            Ok(MockRuntime { id: 3 })
        })
        .await;
    assert!(matches!(result, Err(RegistryAcquireError::ShuttingDown)));
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert!(registry.shutdown_all().await.removed.is_empty());
}
