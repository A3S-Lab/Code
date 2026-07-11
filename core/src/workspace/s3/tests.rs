use super::*;

#[test]
fn key_for_root_uses_prefix_only() {
    let backend = make_backend("ws/u1/s1");
    let key = backend.key_for(&WorkspacePath::root());
    assert_eq!(key, "ws/u1/s1");
}

#[test]
fn key_for_nested_path_joins_with_slash() {
    let backend = make_backend("ws/u1/s1");
    let key = backend.key_for(&WorkspacePath::from_normalized("src/main.rs"));
    assert_eq!(key, "ws/u1/s1/src/main.rs");
}

#[test]
fn key_for_empty_prefix_uses_path_only() {
    let backend = make_backend("");
    assert_eq!(
        backend.key_for(&WorkspacePath::from_normalized("notes.txt")),
        "notes.txt"
    );
    assert_eq!(backend.key_for(&WorkspacePath::root()), "");
}

#[test]
fn list_prefix_root_with_workspace_prefix() {
    let backend = make_backend("ws/u1/s1");
    assert_eq!(backend.list_prefix_for(&WorkspacePath::root()), "ws/u1/s1/");
}

#[test]
fn list_prefix_root_with_empty_workspace_prefix() {
    let backend = make_backend("");
    assert_eq!(backend.list_prefix_for(&WorkspacePath::root()), "");
}

#[test]
fn list_prefix_nested_path() {
    let backend = make_backend("ws/u1/s1");
    let path = WorkspacePath::from_normalized("src");
    assert_eq!(backend.list_prefix_for(&path), "ws/u1/s1/src/");
}

#[test]
fn normalize_prefix_strips_slashes() {
    assert_eq!(normalize_prefix("/foo/bar/"), "foo/bar");
    assert_eq!(normalize_prefix("foo"), "foo");
    assert_eq!(normalize_prefix(""), "");
    assert_eq!(normalize_prefix("/"), "");
}

#[test]
fn strip_dir_name_extracts_immediate_child() {
    assert_eq!(
        strip_dir_name("ws/u1/s1/src/", "ws/u1/s1/"),
        Some("src".to_string())
    );
    assert_eq!(strip_dir_name("ws/u1/s1/", "ws/u1/s1/"), None);
    assert_eq!(strip_dir_name("other/", "ws/u1/s1/"), None);
}

#[test]
fn strip_file_name_rejects_nested_keys() {
    assert_eq!(
        strip_file_name("ws/u1/s1/notes.txt", "ws/u1/s1/"),
        Some("notes.txt".to_string())
    );
    // Nested key — should be claimed by a deeper LIST instead.
    assert_eq!(strip_file_name("ws/u1/s1/src/main.rs", "ws/u1/s1/"), None);
    assert_eq!(strip_file_name("other/notes.txt", "ws/u1/s1/"), None);
}

#[test]
fn config_builder_sets_fields() {
    let cfg = S3BackendConfig::new("bucket", "prefix", "AK", "SK")
        .endpoint("https://minio.local:9000")
        .region("cn-east-1")
        .session_token("TOKEN")
        .force_path_style(true)
        .request_timeout(Duration::from_secs(5))
        .max_read_bytes(4096);
    assert_eq!(cfg.bucket, "bucket");
    assert_eq!(cfg.prefix, "prefix");
    assert_eq!(cfg.endpoint.as_deref(), Some("https://minio.local:9000"));
    assert_eq!(cfg.region.as_deref(), Some("cn-east-1"));
    assert_eq!(cfg.session_token.as_deref(), Some("TOKEN"));
    assert!(cfg.force_path_style);
    assert_eq!(cfg.request_timeout, Some(Duration::from_secs(5)));
    assert_eq!(cfg.max_read_bytes, Some(4096));
}

#[test]
fn config_default_max_read_bytes_is_none_until_set() {
    let cfg = S3BackendConfig::new("bucket", "prefix", "AK", "SK");
    assert!(cfg.max_read_bytes.is_none());
}

#[test]
fn backend_applies_default_max_read_bytes_when_config_omits_it() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK");
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.max_read_bytes(), DEFAULT_MAX_READ_BYTES);
}

#[test]
fn backend_respects_config_max_read_bytes_override() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK").max_read_bytes(2048);
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.max_read_bytes(), 2048);
}

#[test]
fn backend_treats_zero_max_read_bytes_as_default() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK").max_read_bytes(0);
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.max_read_bytes(), DEFAULT_MAX_READ_BYTES);
}

#[test]
fn validate_content_length_allows_within_cap() {
    assert!(validate_content_length(Some(1024), 4096, "bucket", "key").is_ok());
    assert!(validate_content_length(Some(0), 4096, "bucket", "key").is_ok());
    assert!(validate_content_length(Some(4096), 4096, "bucket", "key").is_ok());
}

#[test]
fn validate_content_length_rejects_over_cap() {
    let err = validate_content_length(Some(4097), 4096, "bucket", "ws/big.txt").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("exceeds workspace max_read_bytes"),
        "msg: {msg}"
    );
    assert!(msg.contains("s3://bucket/ws/big.txt"), "msg: {msg}");
}

#[test]
fn validate_content_length_rejects_missing_header() {
    let err = validate_content_length(None, 4096, "bucket", "ws/key").unwrap_err();
    assert!(err.to_string().contains("did not report Content-Length"));
}

#[test]
fn validate_content_length_rejects_negative_length() {
    let err = validate_content_length(Some(-1), 4096, "bucket", "ws/key").unwrap_err();
    assert!(err.to_string().contains("invalid content-length"));
}

#[test]
fn services_s3_factory_disables_exec_search_and_git_by_default() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK");
    let services = super::super::WorkspaceServices::s3(cfg);
    let caps = services.capabilities();
    assert!(caps.read);
    assert!(caps.write);
    assert!(!caps.exec);
    assert!(!caps.search);
    assert!(!caps.git);
    assert!(services.command_runner().is_none());
    assert!(services.search().is_none());
    assert!(services.git().is_none());
    assert!(services.git_stash().is_none());
    assert!(services.git_worktree().is_none());
    assert_eq!(services.operation_timeout(), Some(Duration::from_secs(60)));
}

#[test]
fn services_s3_factory_registers_search_when_enabled() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK").enable_search(true);
    let services = super::super::WorkspaceServices::s3(cfg);
    let caps = services.capabilities();
    assert!(caps.search, "search capability must be on when enabled");
    assert!(
        services.search().is_some(),
        "search provider must be wired when enabled"
    );
    // Disabled providers stay None — opt-in is per-capability, not all-or-nothing.
    assert!(!caps.exec);
    assert!(!caps.git);
}

#[test]
fn config_search_defaults_off_until_enabled() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK");
    assert!(!cfg.search_enabled);
    assert!(cfg.max_objects_scanned.is_none());
    assert!(cfg.max_grep_bytes_per_object.is_none());

    let cfg = cfg
        .enable_search(true)
        .max_objects_scanned(50)
        .max_grep_bytes_per_object(256 * 1024);
    assert!(cfg.search_enabled);
    assert_eq!(cfg.max_objects_scanned, Some(50));
    assert_eq!(cfg.max_grep_bytes_per_object, Some(256 * 1024));
}

#[test]
fn backend_applies_search_defaults_when_config_omits_them() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK").enable_search(true);
    let backend = S3WorkspaceBackend::new(cfg);
    assert!(backend.search_enabled());
    assert_eq!(backend.max_objects_scanned(), DEFAULT_MAX_OBJECTS_SCANNED);
    assert_eq!(
        backend.max_grep_bytes_per_object(),
        DEFAULT_MAX_GREP_BYTES_PER_OBJECT
    );
}

#[test]
fn backend_treats_zero_search_limits_as_defaults() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK")
        .enable_search(true)
        .max_objects_scanned(0)
        .max_grep_bytes_per_object(0)
        .search_concurrency(0);
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.max_objects_scanned(), DEFAULT_MAX_OBJECTS_SCANNED);
    assert_eq!(
        backend.max_grep_bytes_per_object(),
        DEFAULT_MAX_GREP_BYTES_PER_OBJECT
    );
    assert_eq!(backend.search_concurrency(), DEFAULT_SEARCH_CONCURRENCY);
}

#[test]
fn backend_applies_search_concurrency_default() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK").enable_search(true);
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.search_concurrency(), DEFAULT_SEARCH_CONCURRENCY);
}

#[test]
fn backend_respects_search_concurrency_override() {
    let cfg = S3BackendConfig::new("bucket", "ws", "AK", "SK")
        .enable_search(true)
        .search_concurrency(16);
    let backend = S3WorkspaceBackend::new(cfg);
    assert_eq!(backend.search_concurrency(), 16);
}

#[test]
fn join_workspace_path_handles_root_and_nested_bases() {
    let root = WorkspacePath::root();
    let joined = join_workspace_path(&root, "main.rs");
    assert_eq!(joined.as_str(), "main.rs");

    let base = WorkspacePath::from_normalized("src");
    let joined = join_workspace_path(&base, "foo/main.rs");
    assert_eq!(joined.as_str(), "src/foo/main.rs");
}

#[test]
fn basename_returns_last_segment() {
    assert_eq!(basename("src/main.rs"), "main.rs");
    assert_eq!(basename("main.rs"), "main.rs");
    assert_eq!(basename("a/b/c/d.txt"), "d.txt");
}

/// Documents the `glob` crate behaviour the S3 search impl works around.
///
/// `glob::Pattern::matches` is more permissive than the filesystem walker
/// behind `glob::glob`: `*` *does* match across `/`, so `*.rs` matches
/// both `main.rs` and `src/main.rs`. The local backend gets non-recursive
/// semantics for free from the walker; on S3 we have to filter explicitly
/// when the user did not write `**`. This test pins the assumption so a
/// future `glob` crate upgrade with stricter semantics surfaces here
/// rather than silently changing user-visible behaviour.
#[test]
fn glob_pattern_matches_is_permissive_across_slashes() {
    let permissive = glob::Pattern::new("*.rs").unwrap();
    assert!(permissive.matches("main.rs"));
    assert!(
        permissive.matches("src/main.rs"),
        "`glob` crate's `*` matches across `/`; if this ever changes, drop \
             the manual `rel.contains('/')` guard in WorkspaceSearch::glob"
    );

    let recursive = glob::Pattern::new("**/*.rs").unwrap();
    assert!(recursive.matches("src/main.rs"));
    assert!(recursive.matches("main.rs"));
}

fn make_backend(prefix: &str) -> S3WorkspaceBackend {
    let cfg = S3BackendConfig::new("bucket", prefix, "AK", "SK");
    S3WorkspaceBackend::new(cfg)
}
