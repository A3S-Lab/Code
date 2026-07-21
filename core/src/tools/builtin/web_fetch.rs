//! Web fetch tool - Fetch content from URLs

use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{header::HeaderMap, Url};
use std::time::Duration;

use super::safe_http::{
    explicit_web_proxy_from_env, get_with_redirects, parse_http_url, redirect_target,
    safe_url_for_diagnostic, sanitize_fetch_url, system_web_proxy, RedirectQueryPolicy,
    MAX_REDIRECTS,
};

/// Maximum response size (5MB)
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_MAX_CHARS: usize = 50_000;
const MAX_CONTENT_CHARS: usize = 100_000;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and convert to text or markdown. Supports HTML to Markdown conversion. 5MB download size limit and capped tool output. Configurable timeout (max 120 seconds)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Required. The URL to fetch content from. Must start with http:// or https://. Always provide this exact field name: 'url'."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text", "html"],
                    "description": "Optional. Output format. Default: markdown."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Timeout in seconds. Default: 30. Maximum: 120."
                },
                "body_only": {
                    "type": "boolean",
                    "description": "Optional. For HTML responses, extract the body element before conversion. Default: true."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional. Character offset into the converted content. Default: 0."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_CONTENT_CHARS,
                    "description": "Optional. Maximum converted characters to return. Default: 50000; maximum: 100000."
                }
            },
            "required": ["url"],
            "examples": [
                {
                    "url": "https://example.com"
                },
                {
                    "url": "https://example.com",
                    "format": "text",
                    "timeout": 15
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::read_only_paginated(8)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolOutput::error("url parameter is required")),
        };

        let (url, request_url) = match parse_safe_request_url(url) {
            Ok(parsed) => parsed,
            Err(error) => return Ok(ToolOutput::error(error)),
        };

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        let body_only = args
            .get("body_only")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        let offset = match args.get("offset") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) => value,
                None => {
                    return Ok(invalid_fetch_argument(
                        "offset must be a non-negative integer",
                    ))
                }
            },
            None => 0,
        };
        let requested_max_chars = match args.get("max_chars") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) if value > 0 => value,
                _ => {
                    return Ok(invalid_fetch_argument(
                        "max_chars must be a positive integer",
                    ))
                }
            },
            None => DEFAULT_MAX_CHARS,
        };
        let max_chars = requested_max_chars.min(MAX_CONTENT_CHARS);

        let timeout_secs = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .min(120);

        let timeout = Duration::from_secs(timeout_secs);
        let mut configured_proxy = ctx
            .search_config
            .as_ref()
            .and_then(|config| config.headless.as_ref())
            .and_then(|config| config.proxy_url.clone())
            .or_else(explicit_web_proxy_from_env);
        if configured_proxy.is_none() {
            configured_proxy = system_web_proxy().await;
        }
        let page = match tokio::time::timeout(
            timeout,
            fetch_url(url, format, body_only, configured_proxy.as_deref()),
        )
        .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(error)) => {
                let output = ToolOutput::error(&error);
                return Ok(if looks_rate_limited(&error) {
                    output.with_error_kind(ToolErrorKind::RateLimited {
                        retry_after_ms: None,
                    })
                } else {
                    output
                });
            }
            Err(_) => {
                return Ok(ToolOutput::error(format!(
                    "Web fetch timed out after {} seconds",
                    timeout_secs
                ))
                .with_error_kind(ToolErrorKind::Timeout {
                    op: "web_fetch".to_string(),
                    duration_ms: timeout_secs.saturating_mul(1_000),
                }))
            }
        };

        let range = match content_range(&page.content, offset, max_chars) {
            Ok(range) => range,
            Err(error) => return Ok(invalid_fetch_argument(&error)),
        };

        let mut source_anchors = vec![request_url];
        let Some(final_url) = super::safe_http_source_url(page.final_url.as_str()) else {
            return Ok(ToolOutput::error(
                "Final URL could not be normalized into a safe source anchor",
            ));
        };
        if source_anchors.first() != Some(&final_url) {
            source_anchors.push(final_url);
        }
        Ok(
            ToolOutput::success(range.content).with_metadata(serde_json::json!({
                "source_anchors": source_anchors,
                "range": {
                    "offset": offset,
                    "requested_max_chars": requested_max_chars,
                    "applied_max_chars": max_chars,
                    "returned_chars": range.returned_chars,
                    "total_chars": range.total_chars,
                    "next_offset": range.next_offset,
                    "eof": range.next_offset.is_none(),
                    "limit_clamped": requested_max_chars != max_chars,
                },
            })),
        )
    }
}

struct FetchedPage {
    content: String,
    final_url: Url,
}

/// Fetch a URL while validating and pinning DNS results for every redirect hop.
async fn fetch_url(
    mut url: Url,
    format: &str,
    body_only: bool,
    proxy_url: Option<&str>,
) -> std::result::Result<FetchedPage, String> {
    let mut remaining_redirects = MAX_REDIRECTS;
    loop {
        let fetched = get_with_redirects(
            url,
            proxy_url,
            HeaderMap::new(),
            remaining_redirects,
            RedirectQueryPolicy::RemoveSensitive,
        )
        .await?;
        remaining_redirects = remaining_redirects.saturating_sub(fetched.redirects);
        let response = fetched.response;
        url = fetched.final_url;
        let status = response.status();

        if !status.is_success() {
            return Err(format!(
                "HTTP {} for URL: {}",
                status,
                safe_url_for_diagnostic(&url)
            ));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_SIZE as u64)
        {
            return Err(format!(
                "Response too large (max: {} bytes)",
                MAX_RESPONSE_SIZE
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| format!("Failed to read response body: {}", error))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_SIZE {
                return Err(format!(
                    "Response too large (max: {} bytes)",
                    MAX_RESPONSE_SIZE
                ));
            }
            bytes.extend_from_slice(&chunk);
        }

        let body = String::from_utf8_lossy(&bytes).to_string();
        if content_type.contains("text/html") {
            if let Some(location) = html_refresh_location(&body) {
                if remaining_redirects == 0 {
                    return Err(format!(
                        "Too many redirects while fetching URL (max: {})",
                        MAX_REDIRECTS
                    ));
                }
                url = sanitize_fetch_url(redirect_target(&url, &location)?);
                remaining_redirects -= 1;
                continue;
            }
        }
        let body = if body_only && content_type.contains("text/html") {
            extract_html_body(&body).unwrap_or(body)
        } else {
            body
        };
        let content = match format {
            "html" => body,
            "text" if content_type.contains("text/html") => html_to_text(&body),
            "markdown" if content_type.contains("text/html") => html_to_markdown(&body),
            _ if content_type.contains("text/html") => html_to_markdown(&body),
            _ => body,
        };
        return Ok(FetchedPage {
            content,
            final_url: url,
        });
    }
}

struct ContentRange {
    content: String,
    returned_chars: usize,
    total_chars: usize,
    next_offset: Option<usize>,
}

fn content_range(
    content: &str,
    offset: usize,
    max_chars: usize,
) -> std::result::Result<ContentRange, String> {
    let total_chars = content.chars().count();
    if offset > total_chars {
        return Err(format!(
            "offset {offset} exceeds converted content length {total_chars}"
        ));
    }
    let content = content
        .chars()
        .skip(offset)
        .take(max_chars)
        .collect::<String>();
    let returned_chars = content.chars().count();
    let end = offset.saturating_add(returned_chars);
    let next_offset = (end < total_chars).then_some(end);
    let mut content = content;
    if let Some(next_offset) = next_offset {
        content.push_str(&format!(
            "\n\n... (more fetched content available; continue with offset={next_offset})\n"
        ));
    }
    Ok(ContentRange {
        content,
        returned_chars,
        total_chars,
        next_offset,
    })
}

fn extract_html_body(html: &str) -> Option<String> {
    static BODY_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let body_re = BODY_RE.get_or_init(|| {
        regex::Regex::new(r"(?is)<body\b[^>]*>(.*?)</body\s*>").expect("static HTML body regex")
    });
    body_re
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|body| body.as_str().to_string())
}

/// Return the target of an HTML meta-refresh redirect.
///
/// Static documentation hosts commonly return an HTTP 200 page whose only
/// purpose is redirecting a version alias such as `/stable/` to `/current/`.
/// Treat it as a redirect hop so callers receive the actual page instead of a
/// misleading "Redirecting…" document. The resulting URL still passes the
/// same per-hop SSRF validation as an HTTP Location header.
fn html_refresh_location(html: &str) -> Option<String> {
    static META_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ATTR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let meta_re = META_RE
        .get_or_init(|| regex::Regex::new(r"(?is)<meta\b[^>]*>").expect("static meta tag regex"));
    let attr_re = ATTR_RE.get_or_init(|| {
        regex::Regex::new(r#"(?is)\b([a-z][a-z0-9:_-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
            .expect("static HTML attribute regex")
    });

    for tag in meta_re.find_iter(html).map(|matched| matched.as_str()) {
        let mut http_equiv = None;
        let mut content = None;
        for captures in attr_re.captures_iter(tag) {
            let name = captures.get(1)?.as_str().to_ascii_lowercase();
            let value = captures.get(2).or_else(|| captures.get(3))?.as_str().trim();
            match name.as_str() {
                "http-equiv" => http_equiv = Some(value.to_ascii_lowercase()),
                "content" => content = Some(value.to_string()),
                _ => {}
            }
        }
        if http_equiv.as_deref() != Some("refresh") {
            continue;
        }
        let value = content?;
        let (_, target) = value.split_once(';')?;
        let target = target.trim();
        let (directive, target) = target.split_once('=')?;
        if !directive.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        let target = target.trim().trim_matches(['\'', '"']);
        if !target.is_empty() {
            return Some(target.to_string());
        }
    }
    None
}

fn invalid_fetch_argument(message: &str) -> ToolOutput {
    ToolOutput::error(message).with_error_kind(ToolErrorKind::InvalidArgument {
        message: message.to_string(),
    })
}

fn looks_rate_limited(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("429")
        || message.contains("rate limit")
        || message.contains("too many requests")
}

fn parse_safe_request_url(input: &str) -> std::result::Result<(Url, String), String> {
    let request = sanitize_fetch_url(parse_http_url(input)?);
    let safe = super::safe_http_source_url(request.as_str())
        .ok_or_else(|| "URL could not be normalized into a safe source anchor".to_string())?;
    Ok((request, safe))
}

/// Convert HTML to plain text using html2text (handles encoding, tags, scripts, etc.)
fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| String::from("[failed to parse HTML]"))
}

/// Convert HTML to markdown using htmd
fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html_to_text(html))
}

#[cfg(test)]
mod tests {
    use super::super::safe_http::{
        build_proxy_client, parse_macos_proxy, validate_resolved_addresses,
    };
    #[cfg(unix)]
    use super::super::safe_http::{command_output_with_timeout, SYSTEM_PROXY_LOOKUP_TIMEOUT};
    use super::*;
    use std::net::SocketAddr;

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

    #[test]
    fn test_html_to_markdown() {
        let html = "<h1>Title</h1><p>Content here</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Content here"));
        // htmd produces proper markdown headers
        assert!(md.contains("# Title"));
    }
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
