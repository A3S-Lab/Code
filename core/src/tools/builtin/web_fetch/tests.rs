use super::*;

#[test]
fn test_html_to_text_basic() {
    let html = "<p>Hello <b>world</b></p>";
    let text = html_to_text(html);
    assert!(text.contains("Hello"));
    assert!(text.contains("world"));
    assert!(!text.contains("<p>"));
    assert!(!text.contains("<b>"));
}

#[test]
fn test_html_to_text_entities() {
    let html = "foo &amp; bar &lt; baz &gt; qux";
    let text = html_to_text(html);
    assert!(text.contains("foo & bar < baz > qux"));
}

#[test]
fn test_html_to_text_strips_script() {
    let html = "<p>before</p><script>alert('xss')</script><p>after</p>";
    let text = html_to_text(html);
    assert!(text.contains("before"));
    assert!(text.contains("after"));
    assert!(!text.contains("alert"));
}

#[test]
fn test_html_to_text_multibyte_utf8() {
    let html = "<p>Diseño español — café résumé naïve</p>";
    let text = html_to_text(html);
    assert!(text.contains("Diseño"));
    assert!(text.contains("café"));
    assert!(text.contains("résumé"));
}

#[test]
fn test_html_refresh_location_handles_static_doc_redirect() {
    let html = r#"<!doctype html><html>
          <meta http-equiv="refresh" content="0; url=/docs/current/overview.html">
          <h1>Redirecting&hellip;</h1>
        </html>"#;

    assert_eq!(
        html_refresh_location(html).as_deref(),
        Some("/docs/current/overview.html")
    );
}

#[tokio::test]
async fn test_web_fetch_invalid_url() {
    let tool = WebFetchTool;
    let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

    let result = tool
        .execute(&serde_json::json!({"url": "not-a-url"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("must start with"));
}

#[tokio::test]
async fn test_web_fetch_missing_url() {
    let tool = WebFetchTool;
    let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

    let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!result.success);
}

#[test]
fn web_fetch_failure_classification_uses_typed_transport_state_only() {
    let untyped = FetchFailure::untyped(
        "A human-readable page says HTTP 429, rate limit, and too many requests.",
    )
    .into_tool_output();
    assert_eq!(untyped.error_kind, None);

    let rate_limited = FetchFailure::http_status(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        Some(2_500),
        "typed HTTP failure",
    )
    .into_tool_output();
    assert_eq!(
        rate_limited.error_kind,
        Some(ToolErrorKind::RateLimited {
            retry_after_ms: Some(2_500),
        })
    );

    let unavailable = FetchFailure::http_status(
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        None,
        "typed HTTP failure",
    )
    .into_tool_output();
    assert_eq!(
        unavailable.error_kind,
        Some(ToolErrorKind::Transport {
            op: "web_fetch".to_string(),
        })
    );
}

#[tokio::test]
#[ignore = "requires external network"]
async fn real_system_proxy_fetches_official_https_source() {
    let tool = WebFetchTool;
    let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));
    let result = tool
        .execute(
            &serde_json::json!({
                "url": "https://tokio.rs/tokio/tutorial",
                "format": "markdown",
                "timeout": 20
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    assert!(result.content.to_ascii_lowercase().contains("tokio"));
}

#[test]
fn test_web_fetch_schema_is_canonical() {
    let tool = WebFetchTool;
    let params = tool.parameters();
    assert_eq!(params["additionalProperties"], false);
    assert_eq!(params["required"], serde_json::json!(["url"]));
    let examples = params["examples"].as_array().unwrap();
    assert_eq!(examples[0]["url"], "https://example.com");
    assert!(examples[0].get("link").is_none());
}

#[test]
fn test_web_fetch_request_and_diagnostics_use_safe_url() {
    let (request, anchor) = parse_safe_request_url(
        "https://fetch-user:fetch-password@example.com/page?api_key=secret#private",
    )
    .unwrap();

    assert_eq!(request.as_str(), "https://example.com/page");
    assert_eq!(anchor, "https://example.com/page");
    assert_eq!(safe_url_for_diagnostic(&request), anchor);
    for secret in [
        "fetch-user",
        "fetch-password",
        "api_key",
        "secret",
        "private",
    ] {
        assert!(!request.as_str().contains(secret));
        assert!(!anchor.contains(secret));
    }
}

#[test]
fn test_web_fetch_request_preserves_functional_query_but_redacts_anchor() {
    let (request, anchor) = parse_safe_request_url(
            "https://api.example.com/scores?from=2026-07-09&to=2026-07-19&limit=20&api_key=secret#private",
        )
        .unwrap();

    assert_eq!(
        request.as_str(),
        "https://api.example.com/scores?from=2026-07-09&to=2026-07-19&limit=20"
    );
    assert_eq!(anchor, "https://api.example.com/scores");
    assert!(!request.as_str().contains("api_key"));
    assert!(!request.as_str().contains("secret"));
    assert!(!request.as_str().contains("private"));
}

#[test]
fn test_web_fetch_blocks_non_public_literal_hosts() {
    for url in [
        "http://localhost/",
        "http://sub.localhost/",
        "http://127.0.0.1/",
        "http://127.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://10.0.0.1/",
        "http://100.64.0.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://172.16.0.1/",
        "http://192.168.1.1/",
        "http://[::1]/",
        "http://[fc00::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://[2002:7f00:1::]/",
    ] {
        assert!(parse_http_url(url).is_err(), "{url} must be blocked");
    }

    assert!(parse_http_url("https://8.8.8.8/").is_ok());
    assert!(parse_http_url("https://[2606:4700:4700::1111]/").is_ok());
}

#[test]
fn test_web_fetch_revalidates_redirect_targets() {
    let base = parse_http_url("https://example.com/public/page").unwrap();
    assert!(redirect_target(&base, "/next").is_ok());
    assert!(redirect_target(&base, "http://127.0.0.1/admin").is_err());
    assert!(redirect_target(&base, "//169.254.169.254/latest/meta-data").is_err());
    assert!(redirect_target(&base, "file:///etc/passwd").is_err());
}

#[test]
fn test_web_fetch_rejects_mixed_public_private_dns_answers() {
    let addresses = [
        SocketAddr::from(([93, 184, 216, 34], 443)),
        SocketAddr::from(([127, 0, 0, 1], 443)),
    ];
    assert!(validate_resolved_addresses("example.com", &addresses).is_err());
    assert!(validate_resolved_addresses("example.com", &addresses[..1]).is_ok());
    assert!(validate_resolved_addresses("example.com", &[]).is_err());
}

#[test]
fn test_web_fetch_proxy_mode_is_explicit_and_keeps_literal_ssrf_checks() {
    assert!(build_proxy_client("http://127.0.0.1:7897").is_ok());
    assert!(build_proxy_client("not a proxy URL").is_err());
    assert!(parse_http_url("http://127.0.0.1/admin").is_err());
    assert!(parse_http_url("http://169.254.169.254/latest/meta-data").is_err());
    assert!(parse_http_url("https://www.jma.go.jp/").is_ok());
}

#[test]
fn test_macos_system_proxy_parser_prefers_https() {
    let text = r#"
            HTTPEnable : 1
            HTTPPort : 8080
            HTTPProxy : 127.0.0.1
            HTTPSEnable : 1
            HTTPSPort : 7897
            HTTPSProxy : proxy.local
        "#;
    assert_eq!(
        parse_macos_proxy(text).as_deref(),
        Some("http://proxy.local:7897")
    );
    assert_eq!(parse_macos_proxy("HTTPSEnable : 0"), None);
}

#[cfg(unix)]
#[tokio::test]
async fn test_system_proxy_command_timeout_bounds_hanging_process() {
    assert_eq!(
        SYSTEM_PROXY_LOOKUP_TIMEOUT,
        Duration::from_millis(750),
        "macOS proxy detection must retain its bounded production budget"
    );
    let mut command = tokio::process::Command::new("/bin/sleep");
    command.arg("30");
    let started = std::time::Instant::now();

    let output = command_output_with_timeout(command, Duration::from_millis(20)).await;

    assert!(output.is_none());
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "proxy command timeout did not converge promptly: {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_system_proxy_command_timeout_kills_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let leaked = directory.path().join("proxy-probe-leak");
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args([
        "-c",
        &format!("exec 1>&- 2>&-; sleep 0.30; touch '{}'", leaked.display()),
    ]);

    let output = command_output_with_timeout(command, Duration::from_millis(20)).await;

    assert!(output.is_none());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !leaked.exists(),
        "a timed-out system proxy probe must not leave descendants"
    );
}

#[test]
fn test_html_to_markdown() {
    let html = "<h1>Title</h1><p>Content here</p>";
    let md = html_to_markdown(html);
    assert!(md.contains("Title"));
    assert!(md.contains("Content here"));
    // htmd produces proper markdown headers
    assert!(md.contains("# Title"));
}

#[test]
fn content_range_is_unicode_safe_and_resumable() {
    let first = content_range("甲乙丙丁戊", 1, 2).unwrap();
    assert!(first.content.starts_with("乙丙"));
    assert_eq!(first.returned_chars, 2);
    assert_eq!(first.total_chars, 5);
    assert_eq!(first.next_offset, Some(3));

    let second = content_range("甲乙丙丁戊", 3, 2).unwrap();
    assert_eq!(second.content, "丁戊");
    assert_eq!(second.next_offset, None);
}

#[test]
fn html_body_extraction_excludes_head_content() {
    let html = "<html><head><title>Hidden</title></head><body><main>Visible</main></body></html>";
    let body = extract_html_body(html).unwrap();
    assert!(body.contains("Visible"));
    assert!(!body.contains("Hidden"));
}

#[test]
fn html_primary_extraction_prefers_main_over_body_navigation() {
    let html = "<html><body><nav>Noise</nav><main><article>Evidence</article></main><footer>Noise</footer></body></html>";
    let main = extract_html_main(html).unwrap();
    assert!(main.contains("Evidence"));
    assert!(!main.contains("Noise"));
}
