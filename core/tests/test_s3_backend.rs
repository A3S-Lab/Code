//! End-to-end integration test for [`a3s_code_core::S3WorkspaceBackend`].
//!
//! Gated on `A3S_S3_TEST_ENDPOINT` so it does not run in default CI. The
//! hermetic release gate starts `fixtures/s3-compat` and points this test at
//! it. Locally, any S3-compatible endpoint works:
//!
//! ```sh
//! export A3S_S3_TEST_ENDPOINT=http://127.0.0.1:9000
//! export A3S_S3_TEST_REGION=us-east-1
//! export A3S_S3_TEST_ACCESS_KEY_ID=a3s-code-akid
//! export A3S_S3_TEST_SECRET_ACCESS_KEY=a3s-code-test-secret
//! export A3S_S3_TEST_BUCKET=a3s-code-tests
//! export A3S_S3_TEST_FORCE_PATH_STYLE=true
//! cargo test -p a3s-code-core --features s3 --test test_s3_backend -- --ignored
//! ```
//!
//! The test uses a per-run UUID prefix so it never collides with other
//! sessions and cleans up its own keys on success.

#![cfg(feature = "s3")]

use a3s_code_core::tools::{ArtifactStoreLimits, ToolExecutor};
use a3s_code_core::{S3BackendConfig, S3WorkspaceBackend, WorkspaceServices};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

fn env_required(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn live_config() -> Option<S3BackendConfig> {
    let endpoint = env_required("A3S_S3_TEST_ENDPOINT")?;
    let bucket = env_required("A3S_S3_TEST_BUCKET")?;
    let access_key_id = env_required("A3S_S3_TEST_ACCESS_KEY_ID")?;
    let secret_access_key = env_required("A3S_S3_TEST_SECRET_ACCESS_KEY")?;
    let prefix = format!(
        "{}/{}",
        env_required("A3S_S3_TEST_PREFIX").unwrap_or_else(|| "a3s-code-tests".to_string()),
        Uuid::new_v4()
    );

    let mut cfg = S3BackendConfig::new(bucket, prefix, access_key_id, secret_access_key)
        .endpoint(endpoint)
        .request_timeout(Duration::from_secs(10));

    if let Some(region) = env_required("A3S_S3_TEST_REGION") {
        cfg = cfg.region(region);
    } else {
        cfg = cfg.region("us-east-1");
    }
    if let Some(force) = env_required("A3S_S3_TEST_FORCE_PATH_STYLE") {
        cfg = cfg.force_path_style(force.parse().unwrap_or(true));
    } else {
        cfg = cfg.force_path_style(true);
    }
    if let Some(token) = env_required("A3S_S3_TEST_SESSION_TOKEN") {
        cfg = cfg.session_token(token);
    }
    Some(cfg)
}

#[tokio::test]
#[ignore = "requires A3S_S3_TEST_ENDPOINT and friends"]
async fn s3_backend_roundtrips_via_session_executor() {
    let Some(cfg) = live_config() else {
        eprintln!("Skipping: A3S_S3_TEST_ENDPOINT not configured");
        return;
    };
    let prefix_for_cleanup = cfg.prefix.clone();
    let bucket_for_cleanup = cfg.bucket.clone();

    let backend = Arc::new(S3WorkspaceBackend::new(cfg));
    let services = WorkspaceServices::from_s3_backend(Arc::clone(&backend));

    // Capability gating must hide bash/git/search.
    let executor = ToolExecutor::new_with_workspace_services_and_artifact_limits(
        format!("s3://{}/{}", backend.bucket(), backend.prefix()),
        Arc::clone(&services),
        ArtifactStoreLimits::default(),
    );
    let definitions = executor.definitions();
    let names: Vec<&str> = definitions.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"read"), "read must be registered");
    assert!(names.contains(&"write"), "write must be registered");
    assert!(names.contains(&"edit"), "edit must be registered");
    assert!(names.contains(&"patch"), "patch must be registered");
    assert!(names.contains(&"ls"), "ls must be registered");
    assert!(
        !names.contains(&"bash"),
        "bash must NOT be registered for S3 backend"
    );
    assert!(
        !names.contains(&"search"),
        "search must NOT be registered for S3 backend"
    );
    assert!(
        !names.contains(&"git"),
        "git must NOT be registered for S3 backend"
    );

    // Writes are bound to a session. An unbound executor cannot claim a dirty
    // path, so this qualification never reaches the S3 API without one.
    let ctx = executor.registry().context().with_session_id("s3-hermetic");

    // write
    let write = executor
        .execute_with_context(
            "write",
            &json!({ "file_path": "notes/hello.txt", "content": "one\ntwo\n" }),
            &ctx,
        )
        .await
        .expect("write tool dispatched");
    assert_eq!(write.exit_code, 0, "{}", write.output);

    // read
    let read = executor
        .execute_with_context("read", &json!({ "file_path": "notes/hello.txt" }), &ctx)
        .await
        .expect("read tool dispatched");
    assert_eq!(read.exit_code, 0, "{}", read.output);
    assert!(
        read.output.contains("one"),
        "read should return persisted content: {}",
        read.output
    );

    // ls — root should contain "notes" subdirectory
    let ls_root = executor
        .execute_with_context("ls", &json!({ "path": "." }), &ctx)
        .await
        .expect("ls root");
    assert_eq!(ls_root.exit_code, 0, "{}", ls_root.output);
    assert!(
        ls_root.output.contains("notes"),
        "ls / should surface the notes/ prefix: {}",
        ls_root.output
    );

    // ls notes — should contain hello.txt
    let ls_notes = executor
        .execute_with_context("ls", &json!({ "path": "notes" }), &ctx)
        .await
        .expect("ls notes");
    assert_eq!(ls_notes.exit_code, 0, "{}", ls_notes.output);
    assert!(
        ls_notes.output.contains("hello.txt"),
        "ls notes/ should surface hello.txt: {}",
        ls_notes.output
    );

    // edit — replace "one" with "uno"
    let edit = executor
        .execute_with_context(
            "edit",
            &json!({
                "file_path": "notes/hello.txt",
                "old_string": "one",
                "new_string": "uno"
            }),
            &ctx,
        )
        .await
        .expect("edit tool dispatched");
    assert_eq!(edit.exit_code, 0, "{}", edit.output);

    let read_after_edit = executor
        .execute_with_context("read", &json!({ "file_path": "notes/hello.txt" }), &ctx)
        .await
        .expect("read after edit");
    assert!(
        read_after_edit.output.contains("uno"),
        "edit should persist: {}",
        read_after_edit.output
    );

    // patch — apply a unified diff
    let patch = executor
        .execute_with_context(
            "patch",
            &json!({
                "file_path": "notes/hello.txt",
                "diff": "@@ -1,2 +1,2 @@\n uno\n-two\n+dos"
            }),
            &ctx,
        )
        .await
        .expect("patch tool dispatched");
    assert_eq!(patch.exit_code, 0, "{}", patch.output);

    let final_content = executor
        .execute_with_context("read", &json!({ "file_path": "notes/hello.txt" }), &ctx)
        .await
        .expect("read after patch")
        .output;
    assert!(
        final_content.contains("uno") && final_content.contains("dos"),
        "patch should produce uno/dos: {}",
        final_content
    );

    // Cleanup keys under our test prefix.
    cleanup_prefix(backend.client(), &bucket_for_cleanup, &prefix_for_cleanup)
        .await
        .expect("the integration fixture must remove every object it created");
}

async fn cleanup_prefix(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> anyhow::Result<()> {
    let prefix = if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    };
    let mut continuation: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(&prefix);
        if let Some(ref token) = continuation {
            req = req.continuation_token(token);
        }
        let resp = req.send().await?;
        for obj in resp.contents() {
            if let Some(key) = obj.key() {
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await?;
            }
        }
        if resp.is_truncated().unwrap_or(false) {
            continuation = resp.next_continuation_token().map(|s| s.to_string());
            if continuation.is_none() {
                break;
            }
        } else {
            break;
        }
    }

    let remaining = client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(&prefix)
        .send()
        .await?;
    if !remaining.contents().is_empty() {
        anyhow::bail!("S3 integration cleanup left objects under {prefix}");
    }
    Ok(())
}

/// S-S3-01: 40 put/get/delete cycles against the hermetic s3-compat fixture,
/// including 5 oversize puts that must fail closed without leaving objects or
/// leaking credentials in error strings.
#[tokio::test]
#[ignore = "S-S3-01 soak: requires A3S_S3_TEST_ENDPOINT (fixtures/s3-compat)"]
async fn soak_s3_put_get_delete_returns_to_prefix_baseline() {
    let Some(mut cfg) = live_config() else {
        eprintln!("Skipping: A3S_S3_TEST_ENDPOINT not configured");
        return;
    };
    // Tight ceiling so oversize puts are cheap to exercise.
    cfg = cfg.max_read_bytes(1024);
    let access_key = cfg.access_key_id.clone();
    let secret_key = cfg.secret_access_key.clone();
    let prefix_for_cleanup = cfg.prefix.clone();
    let bucket_for_cleanup = cfg.bucket.clone();

    let backend = Arc::new(S3WorkspaceBackend::new(cfg));
    let services = WorkspaceServices::from_s3_backend(Arc::clone(&backend));
    let executor = ToolExecutor::new_with_workspace_services_and_artifact_limits(
        format!("s3://{}/{}", backend.bucket(), backend.prefix()),
        Arc::clone(&services),
        ArtifactStoreLimits::default(),
    );
    let ctx = executor.registry().context().with_session_id("s3-soak");

    async fn object_count(client: &aws_sdk_s3::Client, bucket: &str, prefix: &str) -> usize {
        let prefix = if prefix.ends_with('/') {
            prefix.to_string()
        } else {
            format!("{prefix}/")
        };
        let resp = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(&prefix)
            .send()
            .await
            .expect("list");
        resp.contents().len()
    }

    let baseline = object_count(backend.client(), &bucket_for_cleanup, &prefix_for_cleanup).await;
    const CYCLES: usize = 40;
    const OVERSIZE: usize = 5;

    for index in 0..CYCLES {
        let path = format!("soak/item-{index}.txt");
        if index < OVERSIZE {
            let huge = "Z".repeat(2048);
            let write = executor
                .execute_with_context(
                    "write",
                    &json!({ "file_path": path, "content": huge }),
                    &ctx,
                )
                .await
                .expect("dispatch");
            assert_ne!(
                write.exit_code, 0,
                "oversize put must fail: {}",
                write.output
            );
            assert!(
                write.output.contains("max_read_bytes") || write.output.contains("exceeds"),
                "{}",
                write.output
            );
            assert!(!write.output.contains(&access_key), "leaked access key");
            assert!(!write.output.contains(&secret_key), "leaked secret");
            continue;
        }

        let content = format!("cycle-{index}-payload");
        let write = executor
            .execute_with_context(
                "write",
                &json!({ "file_path": path, "content": content }),
                &ctx,
            )
            .await
            .expect("write");
        assert_eq!(write.exit_code, 0, "{}", write.output);

        let read = executor
            .execute_with_context("read", &json!({ "file_path": path }), &ctx)
            .await
            .expect("read");
        assert_eq!(read.exit_code, 0, "{}", read.output);
        assert!(
            read.output.contains(&format!("cycle-{index}-payload")),
            "{}",
            read.output
        );

        // Delete via overwrite-empty is not delete; use backend delete through write of then list cleanup.
        // Prefer explicit delete if the tool exists; otherwise remove via AWS client key.
        let key = format!("{}/{}", prefix_for_cleanup.trim_end_matches('/'), path);
        backend
            .client()
            .delete_object()
            .bucket(&bucket_for_cleanup)
            .key(&key)
            .send()
            .await
            .expect("delete");
    }

    let remaining = object_count(backend.client(), &bucket_for_cleanup, &prefix_for_cleanup).await;
    assert_eq!(
        remaining, baseline,
        "object count must return to prefix baseline (was {remaining}, baseline {baseline})"
    );

    cleanup_prefix(backend.client(), &bucket_for_cleanup, &prefix_for_cleanup)
        .await
        .expect("final cleanup");
}
