//! Shared HTTP transport for built-in network tools.
//!
//! Redirects are handled manually so every direct connection is preceded by
//! public-address validation and DNS pinning. Explicit proxies remain useful
//! in Fake-IP environments; in that mode the trusted proxy resolves hostnames.
//! When the system resolver returns only Clash/Surge-style Fake-IP addresses
//! (`198.18.0.0/15`) and no proxy is configured, resolution falls back to
//! DNS-over-HTTPS against `1.1.1.1`. DoH answers are still subject to the same
//! public-address SSRF checks before any connection is opened.

use reqwest::header::{HeaderMap, ACCEPT, ACCEPT_ENCODING, LOCATION, RANGE};
use reqwest::{redirect::Policy, Url};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

pub(super) const MAX_REDIRECTS: usize = 10;

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(super) const SYSTEM_PROXY_LOOKUP_TIMEOUT: Duration = Duration::from_millis(750);

/// Bound DoH fallback so Fake-IP recovery cannot stall a fetch indefinitely.
const DOH_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Cloudflare DNS-over-HTTPS JSON endpoint addressed by literal IP so the
/// lookup itself cannot be poisoned by Fake-IP answers for a hostname.
const DOH_ENDPOINT: &str = "https://1.1.1.1/dns-query";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RedirectQueryPolicy {
    Preserve,
    RemoveSensitive,
}

pub(super) struct SafeHttpResponse {
    pub response: reqwest::Response,
    pub final_url: Url,
    pub redirects: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SafeHttpErrorKind {
    Invalid,
    Transport,
}

#[derive(Debug)]
pub(super) struct SafeHttpError {
    kind: SafeHttpErrorKind,
    message: String,
}

impl SafeHttpError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: SafeHttpErrorKind::Invalid,
            message: message.into(),
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: SafeHttpErrorKind::Transport,
            message: message.into(),
        }
    }

    pub(super) fn is_transport(&self) -> bool {
        self.kind == SafeHttpErrorKind::Transport
    }
}

impl fmt::Display for SafeHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SafeHttpError {}

pub(super) fn explicit_web_proxy_from_env() -> Option<String> {
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

#[cfg(target_os = "macos")]
pub(super) async fn system_web_proxy() -> Option<String> {
    let mut command = tokio::process::Command::new("/usr/sbin/scutil");
    command.arg("--proxy");
    let output = command_output_with_timeout(command, SYSTEM_PROXY_LOOKUP_TIMEOUT).await?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|text| parse_macos_proxy(&text))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(super) async fn command_output_with_timeout(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::tools::process::configure_process_group(&mut command);
    let mut child = crate::tools::process::spawn_tokio_with_native_gate(&mut command).ok()?;
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let output = crate::tools::process::read_process_output(&mut child, timeout_ms, None)
        .await
        .ok()?;
    if output.timed_out {
        return None;
    }
    Some(std::process::Output {
        status: output.status?,
        stdout: output.stdout.into_bytes(),
        stderr: output.stderr.into_bytes(),
    })
}

#[cfg(not(target_os = "macos"))]
pub(super) async fn system_web_proxy() -> Option<String> {
    None
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn parse_macos_proxy(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(':')?;
            (name.trim() == key).then(|| value.trim().to_string())
        })
    };
    for (enabled, host, port) in [
        ("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        ("HTTPEnable", "HTTPProxy", "HTTPPort"),
    ] {
        if value(enabled).as_deref() != Some("1") {
            continue;
        }
        let host = value(host)?;
        let port = value(port)?.parse::<u16>().ok()?;
        return Some(format!("http://{host}:{port}"));
    }
    None
}

/// Send one GET and follow a bounded redirect chain.
///
/// The caller owns the timeout and cancellation scope. Dropping this future
/// cancels the in-flight Reqwest request.
pub(super) async fn get_with_redirects(
    mut url: Url,
    proxy_url: Option<&str>,
    mut headers: HeaderMap,
    max_redirects: usize,
    query_policy: RedirectQueryPolicy,
) -> Result<SafeHttpResponse, SafeHttpError> {
    for redirect_count in 0..=max_redirects {
        validate_url_target(&url).map_err(SafeHttpError::invalid)?;
        let client = match proxy_url {
            Some(proxy_url) => build_proxy_client(proxy_url).map_err(SafeHttpError::invalid)?,
            None => {
                let target = resolve_public_target(&url).await?;
                build_direct_client(&target).map_err(SafeHttpError::transport)?
            }
        };
        let response = client
            .get(url.clone())
            .headers(headers.clone())
            .send()
            .await
            .map_err(|error| {
                let detail = error.without_url().to_string();
                SafeHttpError::transport(format!(
                    "Failed to fetch URL {}: {}",
                    safe_url_for_diagnostic(&url),
                    detail
                ))
            })?;

        let status = response.status();
        if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Ok(SafeHttpResponse {
                response,
                final_url: url,
                redirects: redirect_count,
            });
        }
        if redirect_count == max_redirects {
            return Err(SafeHttpError::invalid(format!(
                "Too many redirects while fetching URL (max: {max_redirects})"
            )));
        }

        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| {
                SafeHttpError::invalid(format!(
                    "HTTP {status} redirect is missing a Location header"
                ))
            })?
            .to_str()
            .map_err(|_| SafeHttpError::invalid("Redirect Location header is not valid UTF-8"))?;
        let mut next = redirect_target(&url, location).map_err(SafeHttpError::invalid)?;
        if query_policy == RedirectQueryPolicy::RemoveSensitive {
            next = sanitize_fetch_url(next);
        }
        headers = redirect_headers(&url, &next, &headers);
        url = next;
    }

    unreachable!("redirect loop always returns or continues within its fixed bound")
}

fn redirect_headers(current: &Url, next: &Url, headers: &HeaderMap) -> HeaderMap {
    if same_origin(current, next) {
        return headers.clone();
    }

    // Range and Accept describe the representation, not caller credentials.
    // In particular, If-Range must not cross origins because its validator is
    // scoped to the original resource.
    let mut retained = HeaderMap::new();
    for name in [ACCEPT, ACCEPT_ENCODING, RANGE] {
        if let Some(value) = headers.get(&name) {
            retained.insert(name, value.clone());
        }
    }
    retained
}

pub(super) fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

#[derive(Debug)]
struct ResolvedTarget {
    host: String,
    addresses: Vec<SocketAddr>,
    is_ip_literal: bool,
}

/// Resolve the target and reject the whole result if any address is non-public.
/// Rejecting mixed public/private answers avoids resolver-order dependent bypasses.
///
/// When every local answer is Fake-IP (`198.18.0.0/15`), retry via DoH. Other
/// non-public answers (loopback, RFC1918, link-local, …) stay hard failures so
/// DoH cannot turn a poisoned private answer into a public connect.
async fn resolve_public_target(url: &Url) -> Result<ResolvedTarget, SafeHttpError> {
    validate_url_target(url).map_err(SafeHttpError::invalid)?;

    let serialized_host = url
        .host_str()
        .ok_or_else(|| SafeHttpError::invalid("URL must include a host"))?;
    let host = serialized_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(serialized_host);
    let port = url
        .port_or_known_default()
        .ok_or_else(|| SafeHttpError::invalid("URL must include a valid port"))?;
    let is_ip_literal = host.parse::<IpAddr>().is_ok();
    let mut addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| {
            SafeHttpError::transport(format!("Failed to resolve URL host {host}: {error}"))
        })?
        .collect();
    addresses.sort_unstable();
    addresses.dedup();

    if let Err(error) = validate_resolved_addresses(host, &addresses) {
        if !is_ip_literal && addresses_are_exclusively_fake_ip(&addresses) {
            addresses = resolve_via_dns_over_https(host, port)
                .await
                .map_err(|doh_error| {
                    SafeHttpError::invalid(format!(
                        "{error}; DNS-over-HTTPS fallback also failed: {doh_error}"
                    ))
                })?;
            addresses.sort_unstable();
            addresses.dedup();
            validate_resolved_addresses(host, &addresses).map_err(SafeHttpError::invalid)?;
        } else {
            return Err(SafeHttpError::invalid(error));
        }
    }

    Ok(ResolvedTarget {
        host: serialized_host.to_string(),
        addresses,
        is_ip_literal,
    })
}

/// True when every resolved address sits in the Clash/Surge Fake-IP range.
fn addresses_are_exclusively_fake_ip(addresses: &[SocketAddr]) -> bool {
    !addresses.is_empty() && addresses.iter().all(|address| is_fake_ip(address.ip()))
}

fn is_fake_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_fake_ipv4(v4),
        IpAddr::V6(v6) => v6.to_ipv4().is_some_and(is_fake_ipv4),
    }
}

fn is_fake_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    a == 198 && (b == 18 || b == 19)
}

/// Resolve `host` through Cloudflare DoH JSON. Answers are not trusted until
/// [`validate_resolved_addresses`] runs on the returned sockets.
async fn resolve_via_dns_over_https(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .timeout(DOH_LOOKUP_TIMEOUT)
        .user_agent(concat!("a3s-code/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Failed to initialize DoH client: {error}"))?;

    let mut addresses = Vec::new();
    for record_type in ["A", "AAAA"] {
        let mut endpoint =
            Url::parse(DOH_ENDPOINT).map_err(|error| format!("Invalid DoH endpoint: {error}"))?;
        endpoint
            .query_pairs_mut()
            .append_pair("name", host)
            .append_pair("type", record_type);
        let response = client
            .get(endpoint)
            .header(ACCEPT, "application/dns-json")
            .send()
            .await
            .map_err(|error| format!("DoH {record_type} query failed: {}", error.without_url()))?;
        if !response.status().is_success() {
            return Err(format!(
                "DoH {record_type} query returned HTTP {}",
                response.status()
            ));
        }
        let body = response
            .text()
            .await
            .map_err(|error| format!("DoH {record_type} body read failed: {error}"))?;
        addresses.extend(parse_doh_answer_ips(&body, port)?);
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(format!("DoH returned no addresses for host {host}"));
    }
    Ok(addresses)
}

/// Extract A/AAAA addresses from a Cloudflare-style DNS JSON response body.
fn parse_doh_answer_ips(body: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("DoH response is not valid JSON: {error}"))?;
    let status = value
        .get("Status")
        .and_then(|s| s.as_u64())
        .unwrap_or(u64::MAX);
    // 0 = NOERROR. Empty Answer with NOERROR is fine (other record type).
    if status != 0 {
        return Err(format!("DoH response Status={status} (expected NOERROR)"));
    }
    let Some(answers) = value.get("Answer").and_then(|a| a.as_array()) else {
        return Ok(Vec::new());
    };
    let mut addresses = Vec::new();
    for answer in answers {
        let record_type = answer.get("type").and_then(|t| t.as_u64());
        // 1 = A, 28 = AAAA. Ignore CNAME and other types.
        if !matches!(record_type, Some(1 | 28)) {
            continue;
        }
        let Some(data) = answer.get("data").and_then(|d| d.as_str()) else {
            continue;
        };
        if let Ok(ip) = data.parse::<IpAddr>() {
            addresses.push(SocketAddr::new(ip, port));
        }
    }
    Ok(addresses)
}

/// Create a client for one hop only. Redirects are handled manually so the next
/// host is resolved and validated before a connection is attempted.
fn build_direct_client(target: &ResolvedTarget) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .user_agent(concat!("a3s-code/", env!("CARGO_PKG_VERSION")));

    if !target.is_ip_literal {
        builder = builder.resolve_to_addrs(&target.host, &target.addresses);
    }

    builder
        .build()
        .map_err(|error| format!("Failed to initialize HTTP client: {error}"))
}

/// Build a client only for an explicitly configured proxy. In this mode the
/// proxy resolves hostname targets, avoiding local Fake-IP DNS answers while
/// URL-level literal-host checks still run before every redirect hop.
pub(super) fn build_proxy_client(proxy_url: &str) -> Result<reqwest::Client, String> {
    let proxy = reqwest::Proxy::all(proxy_url)
        .map_err(|error| format!("Invalid configured web proxy URL: {error}"))?;
    reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .proxy(proxy)
        .user_agent(concat!("a3s-code/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("Failed to initialize HTTP proxy client: {error}"))
}

pub(super) fn parse_http_url(input: &str) -> Result<Url, String> {
    let url = Url::parse(input)
        .map_err(|_| "URL must start with http:// or https:// and be valid".to_string())?;
    validate_url_target(&url)?;
    Ok(url)
}

pub(super) fn sanitize_fetch_url(mut url: Url) -> Url {
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);

    let retained = url
        .query_pairs()
        .filter(|(key, _)| !sensitive_query_key(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !retained.is_empty() {
        url.query_pairs_mut().extend_pairs(retained);
    }
    url
}

fn sensitive_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace('-', "_").as_str(),
        "access_token"
            | "api_key"
            | "apikey"
            | "auth"
            | "authorization"
            | "cookie"
            | "credential"
            | "id_token"
            | "key"
            | "password"
            | "passwd"
            | "refresh_token"
            | "secret"
            | "session"
            | "sessionid"
            | "sig"
            | "signature"
            | "token"
            | "x_amz_credential"
            | "x_amz_security_token"
            | "x_amz_signature"
            | "x_goog_credential"
            | "x_goog_signature"
    )
}

pub(super) fn safe_url_for_diagnostic(url: &Url) -> String {
    super::safe_http_source_url(url.as_str()).unwrap_or_else(|| "<redacted URL>".to_string())
}

pub(super) fn redirect_target(base: &Url, location: &str) -> Result<Url, String> {
    let mut target = base
        .join(location)
        .map_err(|error| format!("Invalid redirect URL: {error}"))?;
    target.set_fragment(None);
    if !target.username().is_empty() || target.password().is_some() {
        return Err("Redirect URL user information is not supported".to_string());
    }
    validate_url_target(&target)?;
    Ok(target)
}

/// Perform checks available before DNS resolution. Numeric hosts are normalized
/// by `Url`, so decimal/octal/hex IPv4 spellings cannot bypass the IP checks.
pub(super) fn validate_url_target(url: &Url) -> Result<(), String> {
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
                "URL resolves to a non-public address and was blocked: {address}"
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_resolved_addresses(
    host: &str,
    addresses: &[SocketAddr],
) -> Result<(), String> {
    if addresses.is_empty() {
        return Err(format!("URL host did not resolve to an address: {host}"));
    }
    if let Some(address) = addresses
        .iter()
        .map(SocketAddr::ip)
        .find(|address| is_forbidden_ip(*address))
    {
        return Err(format!(
            "URL host {host} resolves to a non-public address and was blocked: {address}"
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
        || (a == 100 && (64..=127).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

fn is_forbidden_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped_v4) = address.to_ipv4() {
        return is_forbidden_ipv4(mapped_v4);
    }
    let segments = address.segments();
    address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x0064 && segments[1] == 0xff9b)
        || (segments[0] == 0x0100 && segments[1..4] == [0, 0, 0])
        || (segments[0] == 0x2001 && segments[1] == 0x0000)
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || segments[0] == 0x2002
        || (segments[0] & 0xfff0) == 0x3ff0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_ip_detection_covers_clash_surge_range_only() {
        assert!(is_fake_ip(IpAddr::from([198, 18, 0, 1])));
        assert!(is_fake_ip(IpAddr::from([198, 19, 255, 255])));
        assert!(!is_fake_ip(IpAddr::from([127, 0, 0, 1])));
        assert!(!is_fake_ip(IpAddr::from([93, 184, 216, 34])));
        assert!(addresses_are_exclusively_fake_ip(&[SocketAddr::from((
            [198, 18, 0, 95],
            443
        ))]));
        assert!(!addresses_are_exclusively_fake_ip(&[
            SocketAddr::from(([198, 18, 0, 95], 443)),
            SocketAddr::from(([93, 184, 216, 34], 443)),
        ]));
        assert!(!addresses_are_exclusively_fake_ip(&[]));
    }

    #[test]
    fn doh_json_parser_extracts_a_and_aaaa_only() {
        let body = r#"{
            "Status": 0,
            "Answer": [
                {"name": "example.com.", "type": 1, "TTL": 60, "data": "104.20.23.154"},
                {"name": "example.com.", "type": 5, "TTL": 60, "data": "cdn.example."},
                {"name": "example.com.", "type": 28, "TTL": 60, "data": "2606:4700:4700::1111"}
            ]
        }"#;
        let addresses = parse_doh_answer_ips(body, 443).unwrap();
        assert_eq!(
            addresses,
            vec![
                SocketAddr::from(([104, 20, 23, 154], 443)),
                SocketAddr::from(("2606:4700:4700::1111".parse::<IpAddr>().unwrap(), 443)),
            ]
        );
        assert!(parse_doh_answer_ips(r#"{"Status":2}"#, 443).is_err());
        assert!(parse_doh_answer_ips(r#"{"Status":0}"#, 443)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cross_origin_redirect_keeps_representation_headers_only() {
        let current = Url::parse("https://downloads.example/file").unwrap();
        let next = Url::parse("https://cdn.example/file").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/octet-stream".parse().unwrap());
        headers.insert(ACCEPT_ENCODING, "identity".parse().unwrap());
        headers.insert(RANGE, "bytes=0-9".parse().unwrap());
        headers.insert(reqwest::header::IF_RANGE, "\"v1\"".parse().unwrap());
        headers.insert(
            reqwest::header::AUTHORIZATION,
            "Bearer secret".parse().unwrap(),
        );

        let redirected = redirect_headers(&current, &next, &headers);
        assert!(redirected.contains_key(ACCEPT));
        assert!(redirected.contains_key(ACCEPT_ENCODING));
        assert!(redirected.contains_key(RANGE));
        assert!(!redirected.contains_key(reqwest::header::IF_RANGE));
        assert!(!redirected.contains_key(reqwest::header::AUTHORIZATION));
    }

    #[test]
    fn same_origin_redirect_preserves_validator() {
        let current = Url::parse("https://example.com:443/file").unwrap();
        let next = Url::parse("https://EXAMPLE.com/other").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::IF_RANGE, "\"v1\"".parse().unwrap());
        assert!(redirect_headers(&current, &next, &headers).contains_key(reqwest::header::IF_RANGE));
    }

    #[tokio::test]
    async fn proxy_mode_fetch_returns_body_without_direct_dns() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<title>Example Domain</title>"),
            )
            .mount(&server)
            .await;

        let url = Url::parse("http://example.test/page").unwrap();
        let fetched = get_with_redirects(
            url,
            Some(server.uri().as_str()),
            HeaderMap::new(),
            MAX_REDIRECTS,
            RedirectQueryPolicy::Preserve,
        )
        .await
        .expect("proxy fetch");
        let body = fetched.response.text().await.expect("body");
        assert!(body.contains("Example Domain"));
        assert_eq!(fetched.redirects, 0);
    }

    #[tokio::test]
    async fn proxy_mode_follows_same_origin_redirect_chain() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("redirected-ok"))
            .mount(&server)
            .await;

        let url = Url::parse("http://example.test/start").unwrap();
        let fetched = get_with_redirects(
            url,
            Some(server.uri().as_str()),
            HeaderMap::new(),
            MAX_REDIRECTS,
            RedirectQueryPolicy::Preserve,
        )
        .await
        .expect("redirect fetch");
        assert_eq!(fetched.redirects, 1);
        assert!(fetched.final_url.as_str().ends_with("/final"));
        let body = fetched.response.text().await.expect("body");
        assert_eq!(body, "redirected-ok");
    }

    #[tokio::test]
    async fn direct_mode_rejects_loopback_literal_before_connect() {
        let url = Url::parse("http://127.0.0.1/admin").unwrap();
        let err = match get_with_redirects(
            url,
            None,
            HeaderMap::new(),
            MAX_REDIRECTS,
            RedirectQueryPolicy::Preserve,
        )
        .await
        {
            Ok(_) => panic!("loopback must fail closed"),
            Err(error) => error,
        };
        assert!(!err.is_transport());
        assert!(err.to_string().contains("non-public"));
    }
}
