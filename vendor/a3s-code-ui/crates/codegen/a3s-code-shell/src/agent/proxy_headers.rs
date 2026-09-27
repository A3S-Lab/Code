use indexmap::IndexMap;
/// Every authenticated request to cli-chat-proxy (web search, image gen, and any future tools that go through the proxy) must carry these headers.
/// Headers injected: `x-a3s-client-version`: required by the proxy's version-gate check. Uses `client_version` when provided, otherwise falls back to cli-chat-proxy compile-time `CARGO_PKG_VERSION`.
/// `X-A3S-Token-Auth` / `x-authenticateresponse`: required by the cli-chat-proxy auth middleware when the `base_url` is a known proxy URL. Existing entries are never overwritten so callers can pre-set a value.
pub(crate) fn inject_proxy_headers(
    headers: &mut IndexMap<String, String>,
    client_version: Option<&str>,
    alpha_test_key: Option<&str>,
    base_url: &str,
) {
    headers
        .entry("x-grok-client-version".to_string())
        .or_insert_with(|| {
            client_version
                .map(String::from)
                .unwrap_or_else(|| a3s_code_version::VERSION.to_string())
        });
    headers
        .entry("x-grok-client-identifier".to_string())
        .or_insert_with(crate::http::process_client_identifier);
    if crate::util::is_cli_chat_proxy_url(base_url) {
        headers
            .entry("X-A3S-Token-Auth".to_string())
            .or_insert_with(|| "a3s-code-cli".to_string());
        headers
            .entry("x-authenticateresponse".to_string())
            .or_insert_with(|| "authenticate-response".to_string());
        headers
            .entry(crate::http::CLIENT_MODE_HEADER.to_string())
            .or_insert_with(|| crate::http::process_client_mode().to_string());
    }
    let _ = (alpha_test_key, base_url);
}
