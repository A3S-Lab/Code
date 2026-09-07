//! Retry logic for LLM API calls
//!
//! Provides exponential backoff with jitter for transient HTTP errors.
//! Supports `Retry-After` header parsing for rate-limited responses.
//!
//! ## Retryable Status Codes
//!
//! - 408: Request Timeout
//! - 429: Too Many Requests (rate limited)
//! - 500: Internal Server Error
//! - 502: Bad Gateway
//! - 503: Service Unavailable
//! - 504: Gateway Timeout
//! - 529: Overloaded (Anthropic-specific)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use a3s_code::retry::RetryConfig;
//!
//! let config = RetryConfig::default(); // 10 retries, 1s base, 30s max
//! ```

use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const MAX_RETRY_ERROR_BODY_BYTES: usize = 4 * 1024;
/// Hard upper bound for provider retries accepted by the Core loop.
///
/// Retry configuration can be supplied by a host, so allowing an arbitrary
/// `u32` here would turn a transient provider failure into an effectively
/// unbounded resource reservation.
pub const MAX_RETRIES: u32 = 100;

/// Configuration for API retry behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries)
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (cap for exponential growth)
    pub max_delay_ms: u64,
    /// HTTP status codes that trigger a retry
    pub retryable_status_codes: Vec<u16>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 10,
            base_delay_ms: 1000,
            max_delay_ms: 30_000,
            retryable_status_codes: vec![408, 429, 500, 502, 503, 504, 529],
        }
    }
}

impl RetryConfig {
    /// Create a retry config with no retries (disabled)
    pub fn disabled() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Check if a given HTTP status code is retryable
    pub fn is_retryable_status(&self, status: StatusCode) -> bool {
        self.retryable_status_codes.contains(&status.as_u16())
    }

    /// Calculate the delay for a given attempt number (0-indexed)
    ///
    /// Uses exponential backoff: `base_delay * 2^attempt`, capped at `max_delay`.
    /// Adds jitter of ±25% to avoid thundering herd.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exp_delay = self.base_delay_ms.saturating_mul(1u64 << attempt.min(10));
        let capped = exp_delay.min(self.max_delay_ms);

        // Add jitter: ±25%
        let jitter_range = capped / 4;
        let jitter = if jitter_range > 0 {
            // Use system time nanos + attempt as entropy for non-deterministic jitter,
            // so different clients/attempts spread their retries to avoid thundering herd.
            let entropy = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0);
            let jitter_offset = (entropy ^ (attempt as u64).wrapping_mul(0x517cc1b727220a95))
                % (jitter_range * 2 + 1);
            capped - jitter_range + jitter_offset
        } else {
            capped
        };

        Duration::from_millis(jitter)
    }

    /// Parse `Retry-After` header value to get a delay duration.
    ///
    /// Supports:
    /// - Integer seconds (e.g., "5")
    /// - Decimal seconds (e.g., "1.5")
    ///
    /// Returns `None` if the header is missing or unparseable.
    pub fn parse_retry_after(header_value: Option<&str>) -> Option<Duration> {
        let value = header_value?.trim();
        // Try parsing as float seconds (covers both "5" and "1.5")
        if let Ok(seconds) = value.parse::<f64>() {
            if seconds > 0.0 && seconds <= 300.0 {
                return Some(Duration::from_secs_f64(seconds));
            }
        }
        None
    }
}

/// Outcome of a single HTTP attempt, used by the retry loop
#[derive(Debug)]
pub enum AttemptOutcome<T> {
    /// Request succeeded
    Success(T),
    /// Request failed with a retryable error
    Retryable {
        status: StatusCode,
        body: String,
        retry_after: Option<Duration>,
    },
    /// Request failed with a non-retryable error (bail immediately)
    Fatal(anyhow::Error),
}

/// A retry loop that exhausted a typed HTTP response status.
///
/// The rendered body is diagnostic only; callers use the retained status for
/// policy decisions.
#[derive(Debug, thiserror::Error)]
#[error("{terminal_message}")]
pub struct RetryExhaustedError {
    attempts: u32,
    status: StatusCode,
    body: String,
    terminal_message: String,
}

impl RetryExhaustedError {
    pub(crate) fn new(attempts: u32, status: StatusCode, body: impl Into<String>) -> Self {
        let body = bound_retry_body(body.into());
        let terminal_message = format!(
            "LLM API request failed after {attempts} attempts. Last status: {status} Body: {body}"
        );
        Self {
            attempts,
            status,
            body,
            terminal_message,
        }
    }

    /// Construct a typed exhaustion value from a wire status for host-side
    /// adapters and tests. Invalid status values conservatively map to 500.
    pub fn from_status(attempts: u32, status: u16, body: impl Into<String>) -> Self {
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::new(attempts, status, body)
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    /// Number of provider attempts consumed before retry authority stopped.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Last provider HTTP status, exposed without requiring callers to depend
    /// on the internal retry module or parse the rendered diagnostic.
    pub fn status_code(&self) -> u16 {
        self.status.as_u16()
    }

    /// Bounded last provider response body retained for diagnostics.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// A stable marker consumed by the outer Agent boundary. Once this retry
    /// authority has exhausted its budget, replaying the same request through
    /// another fallback or circuit breaker only repeats the same side effect.
    pub(crate) fn non_retryable_message(&self) -> &str {
        &self.terminal_message
    }
}

fn bound_retry_body(body: String) -> String {
    if body.len() <= MAX_RETRY_ERROR_BODY_BYTES {
        return body;
    }
    let mut bounded = String::with_capacity(MAX_RETRY_ERROR_BODY_BYTES);
    for character in body.chars() {
        if bounded.len() + character.len_utf8() + 3 > MAX_RETRY_ERROR_BODY_BYTES {
            break;
        }
        bounded.push(character);
    }
    bounded.push('…');
    bounded
}

/// Return the total number of provider attempts represented by a retry budget.
///
/// A retry budget is expressed as retries after the initial request. Keep the
/// diagnostic projection total even for a hostile `u32::MAX` configuration;
/// overflowing here would turn a bounded accounting value into zero (or panic
/// in debug builds) while the retry loop is still handling its terminal path.
#[inline]
fn total_attempts(max_retries: u32) -> u32 {
    max_retries.saturating_add(1)
}

/// Execute an async operation with retry logic.
///
/// The `operation` closure is called on each attempt and must return an `AttemptOutcome`.
/// On retryable failures, waits with exponential backoff before retrying.
/// On fatal failures or after exhausting retries, returns an error.
pub async fn with_retry<T, F, Fut>(config: &RetryConfig, operation: F) -> anyhow::Result<T>
where
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = AttemptOutcome<T>>,
{
    with_retry_inner(config, None, operation).await
}

/// Execute an async operation with retry logic that can be interrupted while
/// waiting for a provider-directed backoff. The ordinary [`with_retry`] API is
/// intentionally retained for callers without a cancellation scope.
pub async fn with_retry_cancellable<T, F, Fut>(
    config: &RetryConfig,
    cancel_token: &CancellationToken,
    operation: F,
) -> anyhow::Result<T>
where
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = AttemptOutcome<T>>,
{
    with_retry_inner(config, Some(cancel_token), operation).await
}

async fn with_retry_inner<T, F, Fut>(
    config: &RetryConfig,
    cancel_token: Option<&CancellationToken>,
    operation: F,
) -> anyhow::Result<T>
where
    F: Fn(u32) -> Fut,
    Fut: std::future::Future<Output = AttemptOutcome<T>>,
{
    if config.max_retries > MAX_RETRIES {
        anyhow::bail!(
            "retry configuration max_retries={} exceeds the maximum of {}",
            config.max_retries,
            MAX_RETRIES
        );
    }

    let mut last_status = None;
    let mut last_body = String::new();

    for attempt in 0..=config.max_retries {
        if cancel_token.is_some_and(|token| token.is_cancelled()) {
            anyhow::bail!("LLM retry cancelled");
        }

        match operation(attempt).await {
            AttemptOutcome::Success(value) => {
                if attempt > 0 {
                    tracing::info!("LLM API request succeeded after {} retries", attempt);
                }
                return Ok(value);
            }
            AttemptOutcome::Fatal(err) => {
                return Err(err);
            }
            AttemptOutcome::Retryable {
                status,
                body,
                retry_after,
            } => {
                last_status = Some(status);
                last_body = body;

                if attempt < config.max_retries {
                    // Determine delay: prefer Retry-After header, fallback to exponential backoff
                    let delay = retry_after.unwrap_or_else(|| config.delay_for_attempt(attempt));

                    tracing::warn!(
                        "LLM API request failed with {} (attempt {}/{}), retrying in {:?}",
                        status,
                        attempt.saturating_add(1),
                        total_attempts(config.max_retries),
                        delay,
                    );

                    if let Some(cancel_token) = cancel_token {
                        tokio::select! {
                            _ = cancel_token.cancelled() => anyhow::bail!("LLM retry cancelled"),
                            _ = tokio::time::sleep(delay) => {}
                        }
                    } else {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }

    // All retries exhausted
    let status = last_status.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Err(anyhow::Error::new(RetryExhaustedError::new(
        total_attempts(config.max_retries),
        status,
        last_body,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn retry_exhaustion_preserves_typed_http_status() {
        let config = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let error = with_retry::<(), _, _>(&config, |_| async {
            AttemptOutcome::Retryable {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: "opaque provider response".to_string(),
                retry_after: None,
            }
        })
        .await
        .expect_err("the only attempt must fail");

        let exhausted = error
            .downcast_ref::<RetryExhaustedError>()
            .expect("retry exhaustion must retain its typed status");
        assert_eq!(exhausted.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn retry_exhaustion_bounds_provider_body_without_losing_status() {
        let error = RetryExhaustedError::new(
            2,
            StatusCode::SERVICE_UNAVAILABLE,
            "错误".repeat(MAX_RETRY_ERROR_BODY_BYTES),
        );
        assert!(error.to_string().len() <= MAX_RETRY_ERROR_BODY_BYTES + 128);
        assert!(error.to_string().contains("503"));
        assert!(error.to_string().ends_with('…'));
    }

    #[test]
    fn total_attempts_saturates_at_u32_max() {
        assert_eq!(total_attempts(0), 1);
        assert_eq!(total_attempts(10), 11);
        assert_eq!(total_attempts(u32::MAX), u32::MAX);
    }

    #[tokio::test]
    async fn excessive_retry_budget_is_rejected_before_provider_use() {
        let config = RetryConfig {
            max_retries: MAX_RETRIES + 1,
            ..RetryConfig::default()
        };
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_operation = Arc::clone(&calls);
        let error = with_retry::<(), _, _>(&config, move |_| {
            calls_for_operation.fetch_add(1, Ordering::Relaxed);
            async { AttemptOutcome::Success(()) }
        })
        .await
        .expect_err("an excessive retry budget must fail closed");

        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(error.to_string().contains("exceeds the maximum"));
    }

    // ========================================================================
    // RetryConfig unit tests
    // ========================================================================

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 10);
        assert_eq!(config.base_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30_000);
        assert_eq!(
            config.retryable_status_codes,
            vec![408, 429, 500, 502, 503, 504, 529]
        );
    }

    #[test]
    fn test_retry_config_disabled() {
        let config = RetryConfig::disabled();
        assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn test_is_retryable_status() {
        let config = RetryConfig::default();
        assert!(config.is_retryable_status(StatusCode::REQUEST_TIMEOUT)); // 408
        assert!(config.is_retryable_status(StatusCode::TOO_MANY_REQUESTS)); // 429
        assert!(config.is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR)); // 500
        assert!(config.is_retryable_status(StatusCode::BAD_GATEWAY)); // 502
        assert!(config.is_retryable_status(StatusCode::SERVICE_UNAVAILABLE)); // 503
        assert!(config.is_retryable_status(StatusCode::GATEWAY_TIMEOUT)); // 504
                                                                          // 529 is not a standard StatusCode, test via from_u16
        assert!(config.is_retryable_status(StatusCode::from_u16(529).unwrap()));

        // Non-retryable
        assert!(!config.is_retryable_status(StatusCode::OK)); // 200
        assert!(!config.is_retryable_status(StatusCode::BAD_REQUEST)); // 400
        assert!(!config.is_retryable_status(StatusCode::UNAUTHORIZED)); // 401
        assert!(!config.is_retryable_status(StatusCode::FORBIDDEN)); // 403
        assert!(!config.is_retryable_status(StatusCode::NOT_FOUND)); // 404
    }

    #[test]
    fn test_delay_for_attempt_exponential() {
        let config = RetryConfig {
            base_delay_ms: 1000,
            max_delay_ms: 60_000,
            ..Default::default()
        };

        // Attempt 0: ~1000ms (with jitter)
        let d0 = config.delay_for_attempt(0);
        assert!(d0.as_millis() >= 750 && d0.as_millis() <= 1250);

        // Attempt 1: ~2000ms (with jitter)
        let d1 = config.delay_for_attempt(1);
        assert!(d1.as_millis() >= 1500 && d1.as_millis() <= 2500);

        // Attempt 2: ~4000ms (with jitter)
        let d2 = config.delay_for_attempt(2);
        assert!(d2.as_millis() >= 3000 && d2.as_millis() <= 5000);
    }

    #[test]
    fn test_delay_capped_at_max() {
        let config = RetryConfig {
            base_delay_ms: 1000,
            max_delay_ms: 5000,
            ..Default::default()
        };

        // Attempt 10 would be 1024s without cap, should be capped at 5s
        let d = config.delay_for_attempt(10);
        assert!(d.as_millis() <= 6250); // 5000 + 25% jitter
    }

    #[test]
    fn test_delay_zero_base() {
        let config = RetryConfig {
            base_delay_ms: 0,
            max_delay_ms: 1000,
            ..Default::default()
        };
        let d = config.delay_for_attempt(0);
        assert_eq!(d.as_millis(), 0);
    }

    // ========================================================================
    // Retry-After header parsing
    // ========================================================================

    #[test]
    fn test_parse_retry_after_integer() {
        let d = RetryConfig::parse_retry_after(Some("5"));
        assert_eq!(d, Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_parse_retry_after_decimal() {
        let d = RetryConfig::parse_retry_after(Some("1.5"));
        assert_eq!(d, Some(Duration::from_secs_f64(1.5)));
    }

    #[test]
    fn test_parse_retry_after_none() {
        assert_eq!(RetryConfig::parse_retry_after(None), None);
    }

    #[test]
    fn test_parse_retry_after_invalid() {
        assert_eq!(RetryConfig::parse_retry_after(Some("not-a-number")), None);
    }

    #[test]
    fn test_parse_retry_after_negative() {
        assert_eq!(RetryConfig::parse_retry_after(Some("-1")), None);
    }

    #[test]
    fn test_parse_retry_after_zero() {
        assert_eq!(RetryConfig::parse_retry_after(Some("0")), None);
    }

    #[test]
    fn test_parse_retry_after_too_large() {
        // > 300s should be rejected
        assert_eq!(RetryConfig::parse_retry_after(Some("301")), None);
    }

    #[test]
    fn test_parse_retry_after_with_whitespace() {
        let d = RetryConfig::parse_retry_after(Some("  3  "));
        assert_eq!(d, Some(Duration::from_secs(3)));
    }

    // ========================================================================
    // RetryConfig serialization
    // ========================================================================

    #[test]
    fn test_retry_config_serde_roundtrip() {
        let config = RetryConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: RetryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.max_retries, config.max_retries);
        assert_eq!(deserialized.base_delay_ms, config.base_delay_ms);
        assert_eq!(deserialized.max_delay_ms, config.max_delay_ms);
        assert_eq!(
            deserialized.retryable_status_codes,
            config.retryable_status_codes
        );
    }

    #[test]
    fn test_retry_config_deserialize_custom() {
        let json = r#"{"max_retries":5,"base_delay_ms":500,"max_delay_ms":10000,"retryable_status_codes":[429,503]}"#;
        let config: RetryConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 10_000);
        assert_eq!(config.retryable_status_codes, vec![429, 503]);
    }

    // ========================================================================
    // with_retry integration tests
    // ========================================================================

    #[tokio::test]
    async fn test_with_retry_success_first_attempt() {
        let config = RetryConfig::default();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = with_retry(&config, |_attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Success("ok")
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_with_retry_success_after_retries() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 10, // Fast for tests
            max_delay_ms: 50,
            ..Default::default()
        };
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = with_retry(&config, |attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    AttemptOutcome::Retryable {
                        status: StatusCode::TOO_MANY_REQUESTS,
                        body: "rate limited".to_string(),
                        retry_after: None,
                    }
                } else {
                    AttemptOutcome::Success("recovered")
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(call_count.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn test_with_retry_all_retries_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay_ms: 10,
            max_delay_ms: 50,
            ..Default::default()
        };
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: anyhow::Result<&str> = with_retry(&config, |_attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Retryable {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: "service down".to_string(),
                    retry_after: None,
                }
            }
        })
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("3 attempts")); // max_retries(2) + 1
        assert!(err.contains("503"));
        assert!(err.contains("service down"));
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_retry_default_budget_is_ten_retries() {
        let config = RetryConfig::default();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: anyhow::Result<&str> = with_retry(&config, |_attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Retryable {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    body: "gateway timeout".to_string(),
                    retry_after: Some(Duration::from_millis(0)),
                }
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            11,
            "default max_retries=10 means one initial attempt plus ten retries"
        );
        assert!(result.unwrap_err().to_string().contains("11 attempts"));
    }

    #[tokio::test]
    async fn test_with_retry_fatal_error_no_retry() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 10,
            max_delay_ms: 50,
            ..Default::default()
        };
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: anyhow::Result<&str> = with_retry(&config, |_attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Fatal(anyhow::anyhow!("invalid API key"))
            }
        })
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid API key"));
        assert_eq!(call_count.load(Ordering::SeqCst), 1); // No retries for fatal
    }

    #[tokio::test]
    async fn test_with_retry_disabled() {
        let config = RetryConfig::disabled();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result: anyhow::Result<&str> = with_retry(&config, |_attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Retryable {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: "rate limited".to_string(),
                    retry_after: None,
                }
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1); // Only initial attempt, no retries
    }

    #[tokio::test]
    async fn test_with_retry_respects_retry_after_header() {
        let config = RetryConfig {
            max_retries: 1,
            base_delay_ms: 10,
            max_delay_ms: 50,
            ..Default::default()
        };
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let start = tokio::time::Instant::now();
        let result = with_retry(&config, |attempt| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    AttemptOutcome::Retryable {
                        status: StatusCode::TOO_MANY_REQUESTS,
                        body: "rate limited".to_string(),
                        retry_after: Some(Duration::from_millis(100)),
                    }
                } else {
                    AttemptOutcome::Success("ok")
                }
            }
        })
        .await;

        assert!(result.is_ok());
        // Should have waited at least 100ms (the retry-after value)
        assert!(start.elapsed() >= Duration::from_millis(90));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancellable_retry_stops_during_retry_after_backoff() {
        let config = RetryConfig {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            ..Default::default()
        };
        let cancel_token = CancellationToken::new();
        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();
        let cancellation = cancel_token.clone();

        let task = tokio::spawn(async move {
            with_retry_cancellable::<(), _, _>(&config, &cancellation, |_attempt| {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    AttemptOutcome::Retryable {
                        status: StatusCode::TOO_MANY_REQUESTS,
                        body: "rate limited".to_string(),
                        retry_after: Some(Duration::from_secs(300)),
                    }
                }
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_token.cancel();
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancellation must interrupt a long Retry-After wait")
            .expect("retry task must not panic")
            .expect_err("cancelled retry must fail");

        assert_eq!(error.to_string(), "LLM retry cancelled");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
