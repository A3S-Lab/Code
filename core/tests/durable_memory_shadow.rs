//! Active-only durable-memory binding contract (`HARNESS-CONV4` / `CAP-GA1`).
//! Filename retained for historical test discovery; shadow mode is gone.

use a3s_code_core::{
    DurableMemoryMode, DurableMemoryRecallPolicy, DurableMemorySession, SessionOptions,
};
use a3s_memory::repository::{InMemoryRepository, MemoryNamespace};
use std::sync::Arc;

#[test]
fn durable_memory_binding_is_typed_exact_and_active_only() {
    let namespace = MemoryNamespace::try_new("tenant-a", "principal-a", "repo-a").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    let binding = DurableMemorySession::active_recall(
        repository,
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(8, 0.0).unwrap(),
    );

    assert_eq!(binding.namespace(), &namespace);
    assert_eq!(binding.mode(), DurableMemoryMode::ActiveRecall);
    assert!(format!("{binding:?}").contains("ActiveRecall"));

    let options = SessionOptions::new().with_durable_memory(binding.clone());
    assert_eq!(
        options
            .durable_memory
            .as_ref()
            .expect("binding installed")
            .namespace(),
        &namespace
    );
}
