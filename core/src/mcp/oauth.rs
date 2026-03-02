//! MCP OAuth Token Exchange
//!
//! Implements the OAuth 2.0 Client Credentials flow for machine-to-machine
//! authentication with MCP servers that require bearer tokens.

use anyhow::{anyhow, Context, Result};

/// Token response from an OAuth token endpoint.
#[derive(Debug, serde::Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[allow(dead_code)]
    pub token_type: String,
    #[allow(dead_code)]
    pub expires_in: Option<u64>,
    #[allow(dead_code)]
    pub refresh_token: Option<String>,
}

/// Exchange client credentials for an access token (OAuth 2.0 Client Credentials flow).
///
/// Sends a `POST` to `token_url` with:
/// ```text
/// grant_type=client_credentials&client_id=...&client_secret=...&scope=...
/// ```
///
/// Returns the raw `access_token` string on success.
pub async fn exchange_client_credentials(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    scopes: &[String],
) -> Result<String> {
    let client = reqwest::Client::builder()
        .build()
        .context("Failed to build HTTP client for OAuth token exchange")?;

    // Build application/x-www-form-urlencoded body
    let scope_str = scopes.join(" ");
    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("scope", &scope_str),
    ];

    let response = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .with_context(|| format!("OAuth token request to {} failed", token_url))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "OAuth token exchange failed at {} (HTTP {}): {}",
            token_url,
            status,
            body
        ));
    }

    let token_resp: TokenResponse = response
        .json()
        .await
        .context("Failed to parse OAuth token response")?;

    Ok(token_resp.access_token)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_response_deserialize() {
        let json = r#"{
            "access_token": "eyJhbGciOiJSUzI1NiJ9...",
            "token_type": "Bearer",
            "expires_in": 3600
        }"#;
        let resp: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "eyJhbGciOiJSUzI1NiJ9...");
        assert_eq!(resp.token_type, "Bearer");
        assert_eq!(resp.expires_in, Some(3600));
        assert!(resp.refresh_token.is_none());
    }

    #[test]
    fn test_token_response_with_refresh_token() {
        let json = r#"{
            "access_token": "access123",
            "token_type": "Bearer",
            "expires_in": 7200,
            "refresh_token": "refresh456"
        }"#;
        let resp: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "access123");
        assert_eq!(resp.refresh_token, Some("refresh456".to_string()));
    }

    #[tokio::test]
    async fn test_exchange_client_credentials_invalid_url() {
        // Should fail gracefully — not panic
        let result = exchange_client_credentials(
            "http://127.0.0.1:1/token",
            "client_id",
            "client_secret",
            &[],
        )
        .await;
        assert!(result.is_err());
    }
}
