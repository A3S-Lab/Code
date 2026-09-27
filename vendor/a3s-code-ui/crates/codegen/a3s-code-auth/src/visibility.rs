/// Apply auth headers to outbound visibility requests.
/// Implemented by `a3s-code-shell::util::grok_auth_credentials::GrokAuthCredentials`.
/// Shell owns credential construction; data-collector builds the request without importing shell types.
pub trait HttpAuth: Send + Sync {
    fn apply(&self, builder: reqwest::RequestBuilder, base_url: &str) -> reqwest::RequestBuilder;
}
