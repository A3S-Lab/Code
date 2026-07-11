//! Web fetch tool - Fetch content from URLs

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{header::LOCATION, redirect::Policy, Url};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Maximum response size (5MB)
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
/// Maximum number of redirects followed by a single fetch.
const MAX_REDIRECTS: usize = 10;

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

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
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

        let timeout_secs = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .min(120);

        let timeout = Duration::from_secs(timeout_secs);
        let page = match tokio::time::timeout(timeout, fetch_url(url, format)).await {
            Ok(Ok(page)) => page,
            Ok(Err(error)) => return Ok(ToolOutput::error(error)),
            Err(_) => {
                return Ok(ToolOutput::error(format!(
                    "Web fetch timed out after {} seconds",
                    timeout_secs
                )))
            }
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
            ToolOutput::success(page.content).with_metadata(serde_json::json!({
                "source_anchors": source_anchors,
            })),
        )
    }
}

struct FetchedPage {
    content: String,
    final_url: Url,
}

/// Fetch a URL while validating and pinning DNS results for every redirect hop.
async fn fetch_url(mut url: Url, format: &str) -> std::result::Result<FetchedPage, String> {
    for redirect_count in 0..=MAX_REDIRECTS {
        let target = resolve_public_target(&url).await?;
        let client = build_client(&target)?;
        let response = client.get(url.clone()).send().await.map_err(|error| {
            format!(
                "Failed to fetch URL {}: {}",
                safe_url_for_diagnostic(&url),
                error
            )
        })?;

        let status = response.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            if redirect_count == MAX_REDIRECTS {
                return Err(format!(
                    "Too many redirects while fetching URL (max: {})",
                    MAX_REDIRECTS
                ));
            }

            let location = response
                .headers()
                .get(LOCATION)
                .ok_or_else(|| format!("HTTP {} redirect is missing a Location header", status))?
                .to_str()
                .map_err(|_| "Redirect Location header is not valid UTF-8".to_string())?;
            url = redirect_target(&url, location)?;
            continue;
        }

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

    unreachable!("redirect loop always returns or continues within its fixed bound")
}

#[derive(Debug)]
struct ResolvedTarget {
    host: String,
    addresses: Vec<SocketAddr>,
    is_ip_literal: bool,
}

/// Resolve the target and reject the whole result if any address is non-public.
/// Rejecting mixed public/private answers avoids resolver-order dependent bypasses.
async fn resolve_public_target(url: &Url) -> std::result::Result<ResolvedTarget, String> {
    validate_url_target(url)?;

    let serialized_host = url
        .host_str()
        .ok_or_else(|| "URL must include a host".to_string())?;
    let host = serialized_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(serialized_host);
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "URL must include a valid port".to_string())?;
    let mut addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("Failed to resolve URL host {}: {}", host, error))?
        .collect();
    addresses.sort_unstable();
    addresses.dedup();

    validate_resolved_addresses(host, &addresses)?;

    Ok(ResolvedTarget {
        host: serialized_host.to_string(),
        addresses,
        is_ip_literal: host.parse::<IpAddr>().is_ok(),
    })
}

/// Create a client for one hop only. Redirects are handled manually so the next
/// host is resolved and validated before a connection is attempted.
fn build_client(target: &ResolvedTarget) -> std::result::Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .user_agent("a3s-code/0.7");

    // Pin the validated DNS result to close the validation/connect rebinding gap.
    // IP literals do not go through Reqwest's DNS resolver and need no override.
    if !target.is_ip_literal {
        builder = builder.resolve_to_addrs(&target.host, &target.addresses);
    }

    builder
        .build()
        .map_err(|error| format!("Failed to initialize HTTP client: {}", error))
}

fn parse_http_url(input: &str) -> std::result::Result<Url, String> {
    let url = Url::parse(input)
        .map_err(|_| "URL must start with http:// or https:// and be valid".to_string())?;
    validate_url_target(&url)?;
    Ok(url)
}

fn parse_safe_request_url(input: &str) -> std::result::Result<(Url, String), String> {
    let parsed = parse_http_url(input)?;
    let safe = super::safe_http_source_url(parsed.as_str())
        .ok_or_else(|| "URL could not be normalized into a safe source anchor".to_string())?;
    let request = parse_http_url(&safe)?;
    Ok((request, safe))
}

fn safe_url_for_diagnostic(url: &Url) -> String {
    super::safe_http_source_url(url.as_str()).unwrap_or_else(|| "<redacted URL>".to_string())
}

fn redirect_target(base: &Url, location: &str) -> std::result::Result<Url, String> {
    let target = base
        .join(location)
        .map_err(|error| format!("Invalid redirect URL: {}", error))?;
    validate_url_target(&target)?;
    Ok(target)
}

/// Perform checks available before DNS resolution. Numeric hosts are normalized
/// by `Url`, so decimal/octal/hex IPv4 spellings cannot bypass the IP checks.
fn validate_url_target(url: &Url) -> std::result::Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("URL must start with http:// or https://".to_string());
    }

    let serialized_host = url
        .host_str()
        .ok_or_else(|| "URL must include a host".to_string())?;
    let host = serialized_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(serialized_host);
    let normalized_host = host.trim_end_matches('.').to_ascii_lowercase();

    if normalized_host == "localhost" || normalized_host.ends_with(".localhost") {
        return Err("URL host is not publicly routable".to_string());
    }

    if let Ok(address) = host.parse::<IpAddr>() {
        if is_forbidden_ip(address) {
            return Err(format!(
                "URL resolves to a non-public address and was blocked: {}",
                address
            ));
        }
    }

    Ok(())
}

fn validate_resolved_addresses(
    host: &str,
    addresses: &[SocketAddr],
) -> std::result::Result<(), String> {
    if addresses.is_empty() {
        return Err(format!("URL host did not resolve to an address: {}", host));
    }

    if let Some(address) = addresses
        .iter()
        .map(SocketAddr::ip)
        .find(|address| is_forbidden_ip(*address))
    {
        return Err(format!(
            "URL host {} resolves to a non-public address and was blocked: {}",
            host, address
        ));
    }

    Ok(())
}

fn is_forbidden_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_forbidden_ipv4(address),
        IpAddr::V6(address) => is_forbidden_ipv6(address),
    }
}

fn is_forbidden_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();

    a == 0
        || a == 10
        || (a == 100 && (64..=127).contains(&b)) // shared address space (RFC 6598)
        || a == 127
        || (a == 169 && b == 254) // link-local and cloud metadata endpoints
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99) // deprecated 6to4 relay anycast
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19)) // benchmark networks
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224 // multicast, reserved, and limited broadcast
}

fn is_forbidden_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped_v4) = address.to_ipv4() {
        return is_forbidden_ipv4(mapped_v4);
    }

    let segments = address.segments();
    address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00 // unique-local (fc00::/7)
        || (segments[0] & 0xffc0) == 0xfe80 // link-local (fe80::/10)
        || (segments[0] & 0xffc0) == 0xfec0 // deprecated site-local (fec0::/10)
        || (segments[0] == 0x0064 && segments[1] == 0xff9b) // NAT64 well-known/local-use
        || (segments[0] == 0x0100 && segments[1..4] == [0, 0, 0]) // discard-only (100::/64)
        || (segments[0] == 0x2001 && segments[1] == 0x0000) // Teredo (2001::/32)
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
        || segments[0] == 0x2002 // 6to4 embeds an IPv4 target (2002::/16)
        || (segments[0] & 0xfff0) == 0x3ff0 // documentation (3fff::/20)
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
    fn test_html_to_markdown() {
        let html = "<h1>Title</h1><p>Content here</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Content here"));
        // htmd produces proper markdown headers
        assert!(md.contains("# Title"));
    }
}
