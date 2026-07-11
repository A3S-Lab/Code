use super::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_and_backend() -> (MockServer, Arc<RemoteGitBackend>) {
    let server = MockServer::start().await;
    let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
        .bearer_token("test-token")
        .request_timeout(Duration::from_secs(5));
    let backend = RemoteGitBackend::new(cfg).unwrap();
    (server, backend)
}

#[test]
fn config_defaults_are_documented() {
    let cfg = RemoteGitBackendConfig::new("http://localhost", "r");
    assert!(cfg.bearer_token.is_none());
    assert!(cfg.client_cert_pem.is_none());
    assert!(cfg.request_timeout.is_none());
    assert!(cfg.max_diff_bytes.is_none());
    assert!(cfg.max_log_entries.is_none());
}

#[test]
fn endpoint_url_format_matches_rfc() {
    let cfg = RemoteGitBackendConfig::new("http://localhost:8080/", "u1/s1");
    let backend = RemoteGitBackend::new(cfg).unwrap();
    // Trailing slash on base_url is stripped.
    assert_eq!(backend.base_url(), "http://localhost:8080");
    assert_eq!(
        backend.endpoint("status"),
        "http://localhost:8080/v1/repos/u1/s1/git/status"
    );
    assert_eq!(
        backend.endpoint("branches/create"),
        "http://localhost:8080/v1/repos/u1/s1/git/branches/create"
    );
}

#[test]
fn mtls_requires_both_cert_and_key() {
    let cfg = RemoteGitBackendConfig::new("http://localhost", "r").client_cert_pem("/dev/null");
    let err = RemoteGitBackend::new(cfg).unwrap_err();
    assert!(
        err.to_string().contains("client_key_pem"),
        "missing-key error must name the missing field, got: {}",
        err
    );

    let cfg = RemoteGitBackendConfig::new("http://localhost", "r").client_key_pem("/dev/null");
    let err = RemoteGitBackend::new(cfg).unwrap_err();
    assert!(
        err.to_string().contains("client_cert_pem"),
        "missing-cert error must name the missing field, got: {}",
        err
    );
}

#[test]
fn mtls_rejects_invalid_pem_blob() {
    let tmp = tempfile::tempdir().unwrap();
    let cert = tmp.path().join("cert.pem");
    let key = tmp.path().join("key.pem");
    std::fs::write(&cert, b"not a pem").unwrap();
    std::fs::write(&key, b"also not a pem").unwrap();

    let cfg = RemoteGitBackendConfig::new("http://localhost", "r")
        .client_cert_pem(&cert)
        .client_key_pem(&key);
    let err = RemoteGitBackend::new(cfg).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("PEM"),
        "PEM-parse failure must surface clearly, got: {}",
        msg
    );
    assert!(
        msg.contains(cert.to_str().unwrap()),
        "error must include the cert path for debugging, got: {}",
        msg
    );
}

#[test]
fn mtls_accepts_self_signed_pair_from_rcgen() {
    // rcgen produces a valid cert + PKCS#8 key pair; `reqwest::Identity`
    // (rustls-tls backend) should accept the concatenated PEM blob.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("rcgen self-signed cert");
    let tmp = tempfile::tempdir().unwrap();
    let cert_path = tmp.path().join("client.cert.pem");
    let key_path = tmp.path().join("client.key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

    let cfg = RemoteGitBackendConfig::new("http://localhost", "r")
        .bearer_token("t")
        .client_cert_pem(&cert_path)
        .client_key_pem(&key_path);
    let backend =
        RemoteGitBackend::new(cfg).expect("valid rcgen-generated PEM pair must produce a backend");
    // We cannot easily verify the identity is wired into the client without
    // a live mTLS server; the assertion above (construction succeeds) is the
    // contract — invalid material would have errored at `from_pem`.
    assert_eq!(backend.base_url(), "http://localhost");
}

#[test]
fn safe_utf8_truncate_respects_boundaries() {
    // ASCII path
    assert_eq!(safe_utf8_truncate("hello", 3), 3);
    assert_eq!(safe_utf8_truncate("hello", 100), 5);
    // Multi-byte path: "héllo" — 'é' is 2 bytes (0xC3 0xA9)
    let s = "héllo";
    // Truncating at byte 2 lands inside 'é'; rounds down to 1 (after 'h')
    assert_eq!(safe_utf8_truncate(s, 2), 1);
    assert_eq!(safe_utf8_truncate(s, 3), 3);
}

#[tokio::test]
async fn status_happy_path() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/status"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "branch": "main",
            "commit": "abc123",
            "is_worktree": false,
            "is_dirty": true,
            "dirty_count": 3,
        })))
        .mount(&server)
        .await;

    let status = backend.status().await.unwrap();
    assert_eq!(status.branch, "main");
    assert_eq!(status.commit, "abc123");
    assert!(status.is_dirty);
    assert_eq!(status.dirty_count, 3);
}

#[tokio::test]
async fn log_respects_client_max_log_entries() {
    let server = MockServer::start().await;
    let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
        .bearer_token("t")
        .max_log_entries(5);
    let backend = RemoteGitBackend::new(cfg).unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/log"))
        .and(wiremock::matchers::body_json(json!({"max_count": 5})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commits": [
                {"id":"a","message":"m","author":"x","date":"d"}
            ]
        })))
        .mount(&server)
        .await;

    // Client asks for 100, but the server should see the capped value.
    let commits = backend.log(100).await.unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].id, "a");
}

#[tokio::test]
async fn list_branches_maps_response() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/branches"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "branches": [
                {"name":"main", "is_current":true},
                {"name":"feat/x"}
            ]
        })))
        .mount(&server)
        .await;

    let branches = backend.list_branches().await.unwrap();
    assert_eq!(branches.len(), 2);
    assert!(branches[0].is_current);
    assert!(!branches[1].is_current);
}

#[tokio::test]
async fn create_branch_succeeds_on_201() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/branches/create"))
        .and(wiremock::matchers::body_json(json!({
            "name":"feat/x","base":"main"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    backend
        .create_branch(WorkspaceGitCreateBranchRequest {
            name: "feat/x".into(),
            base: "main".into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn create_branch_409_yields_remote_git_conflict() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/branches/create"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error":{"code":"BRANCH_EXISTS","message":"branch 'feat/x' already exists"}
        })))
        .mount(&server)
        .await;

    let err = backend
        .create_branch(WorkspaceGitCreateBranchRequest {
            name: "feat/x".into(),
            base: "main".into(),
        })
        .await
        .unwrap_err();
    let conflict = err
        .downcast_ref::<RemoteGitConflict>()
        .expect("409 must downcast to RemoteGitConflict");
    assert_eq!(conflict.code, "BRANCH_EXISTS");
    assert!(conflict.message.contains("feat/x"));
}

#[tokio::test]
async fn checkout_returns_stdout() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/checkout"))
        .and(wiremock::matchers::body_json(json!({
            "refspec":"feat/x","force":false
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stdout":"Switched to branch 'feat/x'"
        })))
        .mount(&server)
        .await;

    let out = backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: "feat/x".into(),
            force: false,
        })
        .await
        .unwrap();
    assert!(out.stdout.contains("feat/x"));
}

#[tokio::test]
async fn checkout_409_dirty_yields_conflict() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/checkout"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error":{"code":"WORKING_TREE_DIRTY","message":"please stash first"}
        })))
        .mount(&server)
        .await;

    let err = backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: "main".into(),
            force: false,
        })
        .await
        .unwrap_err();
    let c = err.downcast_ref::<RemoteGitConflict>().unwrap();
    assert_eq!(c.code, "WORKING_TREE_DIRTY");
}

#[tokio::test]
async fn diff_passes_target_through_and_surfaces_server_truncation() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/diff"))
        .and(wiremock::matchers::body_json(json!({"target":"main"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "diff":"<huge diff>",
            "truncated": true
        })))
        .mount(&server)
        .await;

    let diff = backend
        .diff(WorkspaceGitDiffRequest {
            target: Some("main".to_string()),
        })
        .await
        .unwrap();
    assert!(diff.contains("truncated by gitserver"));
}

#[tokio::test]
async fn diff_enforces_client_max_diff_bytes() {
    let server = MockServer::start().await;
    let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
        .bearer_token("t")
        .max_diff_bytes(8);
    let backend = RemoteGitBackend::new(cfg).unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/diff"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "diff":"AAAAAAAAAAAAAAAAAAAAAA",   // 22 bytes
            "truncated": false
        })))
        .mount(&server)
        .await;

    let diff = backend
        .diff(WorkspaceGitDiffRequest { target: None })
        .await
        .unwrap();
    assert!(diff.contains("truncated by client max_diff_bytes"));
    // First 8 bytes preserved.
    assert!(diff.starts_with("AAAAAAAA"));
}

/// Phase 6.2 OOM defence: the gitserver advertises a Content-Length far
/// beyond what the client tolerates. The request must fail without
/// consuming the body.
///
/// `max_diff_bytes = 8` ⇒ `hard_cap = max(8 * 4, 64 KiB) = 64 KiB`.
/// We respond with `Content-Length: 1 048 576` so the eager rejection
/// path fires.
#[tokio::test]
async fn diff_rejects_oversized_content_length_upfront() {
    let server = MockServer::start().await;
    let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
        .bearer_token("t")
        .max_diff_bytes(8);
    let backend = RemoteGitBackend::new(cfg).unwrap();

    // 1 MiB body — far past the 64 KiB hard cap floor.
    let huge_body = vec![b'A'; 1024 * 1024];
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/diff"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_bytes(huge_body),
        )
        .mount(&server)
        .await;

    let err = backend
        .diff(WorkspaceGitDiffRequest { target: None })
        .await
        .expect_err("oversized body must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Content-Length") && msg.contains("exceeds client cap"),
        "expected eager Content-Length rejection, got: {}",
        msg
    );
}

/// Phase 6.2 OOM defence layer 2: when Content-Length is absent or the
/// server lies about it, the stream-bound accumulator must abort once
/// the cap is exceeded. We use chunked transfer (no Content-Length) so
/// the eager path doesn't fire.
#[tokio::test]
async fn diff_aborts_mid_stream_on_cap_exceeded() {
    let server = MockServer::start().await;
    let cfg = RemoteGitBackendConfig::new(server.uri(), "test")
        .bearer_token("t")
        .max_diff_bytes(8);
    let backend = RemoteGitBackend::new(cfg).unwrap();

    // Body large enough to exceed the 64 KiB hard cap floor; chunked
    // transfer encoded so no Content-Length header is set.
    let big_body = vec![b'A'; 256 * 1024];
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/diff"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("transfer-encoding", "chunked")
                .set_body_bytes(big_body),
        )
        .mount(&server)
        .await;

    let err = backend
        .diff(WorkspaceGitDiffRequest { target: None })
        .await
        .expect_err("oversized streamed body must be rejected");
    let msg = err.to_string();
    // Either the eager path (if wiremock surfaces a Content-Length) or
    // the stream-abort path fires; both are valid defences.
    assert!(
        msg.contains("exceeds client cap")
            || msg.contains("exceeded client cap")
            || msg.contains("Content-Length"),
        "expected oversize rejection, got: {}",
        msg
    );
}

#[tokio::test]
async fn list_remotes_defaults_direction() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/remotes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "remotes":[{"name":"origin","url":"git@x:y.git"}]
        })))
        .mount(&server)
        .await;

    let rs = backend.list_remotes().await.unwrap();
    assert_eq!(rs.len(), 1);
    assert_eq!(rs[0].direction, "fetch");
}

#[tokio::test]
async fn is_repository_returns_bool() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/exists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "is_repository": true
        })))
        .mount(&server)
        .await;

    assert!(backend.is_repository().await.unwrap());
}

#[tokio::test]
async fn list_stashes_maps_response() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/stashes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stashes":[{"index":0,"message":"WIP"}]
        })))
        .mount(&server)
        .await;

    let s = backend.list_stashes().await.unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].message, "WIP");
}

#[tokio::test]
async fn stash_create_409_nothing_to_stash() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/stashes/create"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error":{"code":"NOTHING_TO_STASH","message":"clean tree"}
        })))
        .mount(&server)
        .await;

    let err = backend
        .stash(WorkspaceGitStashRequest {
            message: None,
            include_untracked: false,
        })
        .await
        .unwrap_err();
    let c = err.downcast_ref::<RemoteGitConflict>().unwrap();
    assert_eq!(c.code, "NOTHING_TO_STASH");
}

#[tokio::test]
async fn not_found_404_is_generic_anyhow() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/status"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error":{"code":"REPO_NOT_FOUND","message":"unknown repo"}
        })))
        .mount(&server)
        .await;

    let err = backend.status().await.unwrap_err();
    assert!(err.to_string().contains("not found"), "msg: {}", err);
    assert!(err.downcast_ref::<RemoteGitConflict>().is_none());
}

#[tokio::test]
async fn auth_failure_401_is_generic_anyhow() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/status"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error":{"code":"INVALID_TOKEN","message":"bad bearer"}
        })))
        .mount(&server)
        .await;

    let err = backend.status().await.unwrap_err();
    assert!(err.to_string().contains("auth failed"), "msg: {}", err);
    assert!(err.downcast_ref::<RemoteGitConflict>().is_none());
}

#[tokio::test]
async fn server_500_is_generic_anyhow() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/status"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let err = backend.status().await.unwrap_err();
    assert!(err.to_string().contains("server error"), "msg: {}", err);
}

#[tokio::test]
async fn non_json_error_body_falls_back_to_http_code() {
    let (server, backend) = server_and_backend().await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/test/git/status"))
        .respond_with(ResponseTemplate::new(409).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = backend.status().await.unwrap_err();
    // 409 always yields a conflict — even when the body is opaque, we
    // surface it so callers can detect it; the code falls back to
    // HTTP_409.
    let c = err
        .downcast_ref::<RemoteGitConflict>()
        .expect("409 must yield conflict regardless of body shape");
    assert_eq!(c.code, "HTTP_409");
    assert_eq!(c.message, "not json");
}

#[tokio::test]
async fn with_remote_git_wires_git_and_stash() {
    let services = super::super::WorkspaceServices::local(std::env::temp_dir());
    let upgraded = services
        .with_remote_git(RemoteGitBackendConfig::new("http://localhost", "r").bearer_token("t"))
        .unwrap();
    assert!(upgraded.git().is_some());
    assert!(upgraded.git_stash().is_some());
    // Worktree provider intentionally dropped on remote-git workspaces —
    // worktrees do not have a remote analogue (see RFC §8).
    assert!(upgraded.git_worktree().is_none());
    assert!(upgraded.capabilities().git);
}

/// Regression test for Phase 6.1 field-loss bug.
///
/// `with_remote_git` previously rebuilt `WorkspaceServices` via the
/// builder, which silently dropped `local_root` (and would silently
/// drop any future field). After the fix it goes through
/// `with_git_provider`, which uses an explicit struct literal — the
/// compiler now forces every field to be addressed.
#[tokio::test]
async fn with_remote_git_preserves_local_root_and_unrelated_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let base = super::super::WorkspaceServices::local(temp.path());
    assert!(
        base.local_root().is_some(),
        "precondition: local() must set local_root"
    );
    assert!(
        base.command_runner().is_some(),
        "precondition: local() must wire bash runner"
    );
    let base_root = base.local_root().map(|p| p.to_path_buf());

    let upgraded = base
        .with_remote_git(RemoteGitBackendConfig::new("http://localhost", "r").bearer_token("t"))
        .unwrap();

    // The git provider IS replaced.
    assert!(upgraded.git().is_some());
    assert!(upgraded.capabilities().git);
    // Unrelated capabilities survive.
    assert_eq!(
        upgraded.local_root().map(|p| p.to_path_buf()),
        base_root,
        "local_root must survive with_remote_git"
    );
    assert!(
        upgraded.command_runner().is_some(),
        "command_runner must survive with_remote_git"
    );
    assert!(
        upgraded.search().is_some(),
        "search provider must survive with_remote_git"
    );
    // But worktree is intentionally severed alongside the git swap.
    assert!(upgraded.git_worktree().is_none());
}

/// End-to-end test: drive the built-in `git` tool against a wiremock-backed
/// gitserver. Exercises the full path `git tool → WorkspaceGit (remote) →
/// HTTP → wiremock → JSON → DTO → WorkspaceGitStatus → tool output`.
///
/// This is the contract test for Phase 4.2: if any layer breaks, this
/// test fails. Per-method unit tests above isolate the HTTP layer; this
/// one proves the tool wiring actually works through a real ToolContext.
#[tokio::test]
async fn git_tool_status_works_through_remote_backend() {
    use crate::tools::{Tool, ToolContext};

    let server = MockServer::start().await;
    // `git` tool probes `is_repository` before dispatching.
    Mock::given(method("POST"))
        .and(path("/v1/repos/u1/s1/git/exists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"is_repository": true})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/repos/u1/s1/git/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "branch":"main",
            "commit":"deadbeef",
            "is_worktree": false,
            "is_dirty": false,
            "dirty_count": 0,
        })))
        .mount(&server)
        .await;

    let base = super::super::WorkspaceServices::local(std::env::temp_dir());
    let services = base
        .with_remote_git(RemoteGitBackendConfig::new(server.uri(), "u1/s1").bearer_token("tok"))
        .unwrap();

    let tool = crate::tools::builtin::git::GitTool;
    let ctx = ToolContext::new(std::env::temp_dir()).with_workspace_services(services);

    let result = tool
        .execute(&json!({"command": "status"}), &ctx)
        .await
        .unwrap();
    assert!(result.success, "tool output: {}", result.content);
    assert!(
        result.content.contains("main"),
        "expected branch name in output: {}",
        result.content
    );
    assert!(
        result.content.contains("deadbeef"),
        "expected commit hash in output: {}",
        result.content
    );
    assert!(
        result.content.contains("clean"),
        "expected clean status in output: {}",
        result.content
    );
}
