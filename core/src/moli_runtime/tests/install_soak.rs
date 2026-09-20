//! S-MO-01: ten concurrent installs share one binary, then a bad digest does not replace it.

use super::{fixture_archive, test_config};
use crate::moli_runtime::ensure_moli_from;
use fs2::FileExt;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const CALLERS: usize = 10;

#[tokio::test]
#[ignore = "S-MO-01 soak: 10 concurrent Moli installs and a digest mismatch"]
async fn soak_concurrent_moli_install_publishes_one_binary() {
    let server = MockServer::start().await;
    let (archive, format) = fixture_archive();
    let digest = super::fixture_digest(&archive);
    let target = super::current_target().expect("supported test target");
    let asset_name = format!(
        "moli-{target}.{}",
        if format == "zip" { "zip" } else { "tar.gz" }
    );
    Mock::given(method("GET"))
        .and(wiremock::matchers::path(format!("/{asset_name}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(archive.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let cache = tempfile::tempdir().unwrap();
    let config = Arc::new(test_config(&cache, "9.9.7", &digest));
    let mut tasks = Vec::new();
    for _ in 0..CALLERS {
        let config = Arc::clone(&config);
        let base = server.uri();
        tasks.push(tokio::spawn(async move {
            ensure_moli_from(&config, Duration::from_secs(20), Some(&base), true).await
        }));
    }
    let mut paths = Vec::new();
    for task in tasks {
        paths.push(task.await.unwrap().expect("install"));
    }
    server.verify().await;
    let installed = paths[0].clone();
    assert!(paths.iter().all(|path| path == &installed));
    let installed_bytes = tokio::fs::read(&installed).await.unwrap();

    let lock_path = cache.path().join("moli").join(".install.lock");
    let lock = std::fs::File::open(&lock_path).expect("install lock still exists");
    lock.try_lock_exclusive()
        .expect("install lock was released after the callers finished");
    lock.unlock().unwrap();

    let mismatch = "ab".repeat(32);
    let mismatch_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path(format!("/{asset_name}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(archive))
        .expect(1)
        .mount(&mismatch_server)
        .await;
    let mismatch_config = test_config(&cache, "9.9.7", &mismatch);
    let error = ensure_moli_from(
        &mismatch_config,
        Duration::from_secs(20),
        Some(&mismatch_server.uri()),
        true,
    )
    .await
    .expect_err("digest mismatch must not publish");
    assert!(error.to_string().contains("SHA-256 mismatch"));
    mismatch_server.verify().await;
    assert_eq!(tokio::fs::read(&installed).await.unwrap(), installed_bytes);

    let mut binaries = 0usize;
    let mut stack = vec![cache.path().join("moli")];
    while let Some(directory) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if entry.file_type().await.unwrap().is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "moli" || name == "moli.exe")
            {
                binaries += 1;
            }
        }
    }
    assert_eq!(binaries, 1, "mismatch must not leave a second binary");
}
