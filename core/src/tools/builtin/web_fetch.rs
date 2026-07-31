//! Web fetch tool - Fetch content from URLs

use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{
    header::{HeaderMap, RETRY_AFTER},
    StatusCode, Url,
};
use std::time::Duration;

mod pdf;

use super::safe_http::{
    explicit_web_proxy_from_env, get_with_redirects, parse_http_url, redirect_target,
    safe_url_for_diagnostic, sanitize_fetch_url, system_web_proxy, RedirectQueryPolicy,
    SafeHttpError, MAX_REDIRECTS,
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
        "Fetch content from a URL and convert to text or markdown. Supports HTML to Markdown conversion and text extraction from PDF documents. 5MB download size limit and capped tool output. Configurable timeout (max 120 seconds)."
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
                    "description": "Optional. For HTML responses, extract the semantic main element when present, otherwise the body element, before conversion. Default: true."
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
            Ok(Err(error)) => return Ok(error.into_tool_output()),
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
                "document_kind": page.document_kind,
                "content_type": page.content_type,
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
    document_kind: &'static str,
    content_type: String,
}

#[derive(Debug)]
struct FetchFailure {
    message: String,
    error_kind: Option<ToolErrorKind>,
}

impl FetchFailure {
    fn untyped(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error_kind: None,
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error_kind: Some(ToolErrorKind::Transport {
                op: "web_fetch".to_string(),
            }),
        }
    }

    fn http_status(
        status: StatusCode,
        retry_after_ms: Option<u64>,
        message: impl Into<String>,
    ) -> Self {
        let error_kind = if status == StatusCode::TOO_MANY_REQUESTS {
            Some(ToolErrorKind::RateLimited { retry_after_ms })
        } else if status == StatusCode::REQUEST_TIMEOUT || status.is_server_error() {
            Some(ToolErrorKind::Transport {
                op: "web_fetch".to_string(),
            })
        } else {
            None
        };
        Self {
            message: message.into(),
            error_kind,
        }
    }

    fn into_tool_output(self) -> ToolOutput {
        let output = ToolOutput::error(self.message);
        match self.error_kind {
            Some(error_kind) => output.with_error_kind(error_kind),
            None => output,
        }
    }
}

impl From<String> for FetchFailure {
    fn from(message: String) -> Self {
        Self::untyped(message)
    }
}

impl From<SafeHttpError> for FetchFailure {
    fn from(error: SafeHttpError) -> Self {
        if error.is_transport() {
            Self::transport(error.to_string())
        } else {
            Self::untyped(error.to_string())
        }
    }
}

/// Fetch a URL while validating and pinning DNS results for every redirect hop.
async fn fetch_url(
    mut url: Url,
    format: &str,
    body_only: bool,
    proxy_url: Option<&str>,
) -> std::result::Result<FetchedPage, FetchFailure> {
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
            let retry_after_ms = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after_ms);
            return Err(FetchFailure::http_status(
                status,
                retry_after_ms,
                format!("HTTP {} for URL: {}", status, safe_url_for_diagnostic(&url)),
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
            return Err(FetchFailure::untyped(format!(
                "Response too large (max: {} bytes)",
                MAX_RESPONSE_SIZE
            )));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                FetchFailure::transport(format!("Failed to read response body: {error}"))
            })?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_SIZE {
                return Err(FetchFailure::untyped(format!(
                    "Response too large (max: {} bytes)",
                    MAX_RESPONSE_SIZE
                )));
            }
            bytes.extend_from_slice(&chunk);
        }

        let is_pdf = pdf::response_is_pdf(&content_type, &bytes);
        let is_html = !is_pdf
            && content_type
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/html"));
        let document_kind = if is_pdf {
            "pdf"
        } else if is_html {
            "html"
        } else {
            "text"
        };
        let html_body = is_html.then(|| String::from_utf8_lossy(&bytes).into_owned());
        if let Some(body) = html_body.as_deref() {
            if let Some(location) = html_refresh_location(body) {
                if remaining_redirects == 0 {
                    return Err(FetchFailure::untyped(format!(
                        "Too many redirects while fetching URL (max: {})",
                        MAX_REDIRECTS
                    )));
                }
                url = sanitize_fetch_url(redirect_target(&url, &location)?);
                remaining_redirects -= 1;
                continue;
            }
        }
        let body = if is_pdf {
            pdf::extract_text(bytes).await?
        } else {
            let body = html_body.unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
            if body_only && is_html {
                extract_html_main(&body)
                    .or_else(|| extract_html_body(&body))
                    .unwrap_or(body)
            } else {
                body
            }
        };
        let content = match format {
            "html" => body,
            "text" if is_html => html_to_text(&body),
            "markdown" if is_html => html_to_markdown(&body),
            _ if is_html => html_to_markdown(&body),
            _ => body,
        };
        return Ok(FetchedPage {
            content,
            final_url: url,
            document_kind,
            content_type: content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
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

fn extract_html_main(html: &str) -> Option<String> {
    static MAIN_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let main_re = MAIN_RE.get_or_init(|| {
        regex::Regex::new(r"(?is)<main\b[^>]*>(.*?)</main\s*>").expect("static HTML main regex")
    });
    main_re
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|main| main.as_str().to_string())
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

fn parse_retry_after_ms(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000))
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
#[path = "web_fetch/tests.rs"]
mod tests;
