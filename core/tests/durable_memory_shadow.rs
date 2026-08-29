use a3s_code_core::{DurableMemoryMode, DurableMemorySession, SessionOptions};
use a3s_memory::repository::{InMemoryRepository, MemoryNamespace};
use std::sync::Arc;

#[test]
fn durable_memory_binding_is_typed_exact_and_shadow_only() {
    let namespace = MemoryNamespace::try_new("tenant-a", "principal-a", "repo-a").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    let binding = DurableMemorySession::shadow(repository, namespace.clone());

    assert_eq!(binding.namespace(), &namespace);
    assert_eq!(binding.mode(), DurableMemoryMode::ShadowCandidates);
    assert!(format!("{binding:?}").contains("ShadowCandidates"));

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
