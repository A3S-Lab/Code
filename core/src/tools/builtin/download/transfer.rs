use super::super::safe_http::{
    get_with_redirects, same_origin, RedirectQueryPolicy, SafeHttpResponse, MAX_REDIRECTS,
};
use futures::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_LENGTH,
    CONTENT_RANGE, CONTENT_TYPE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE, RETRY_AFTER,
};
use reqwest::{Response, StatusCode, Url};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const RETRY_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY_MS: u64 = 200;
const MIN_CHUNK_SIZE: u64 = 2 * 1024 * 1024;
const RANGE_ALIGNMENT: u64 = 4 * 1024;
pub(super) const MAX_CONNECTIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureKind {
    Cancelled,
    RateLimited(Option<u64>),
    TooLarge,
    InvalidArgument,
    Network,
    Protocol,
    Io,
}

#[derive(Debug)]
pub(super) struct DownloadFailure {
    pub kind: FailureKind,
    pub message: String,
}

impl DownloadFailure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(super) fn cancelled() -> Self {
        Self::new(FailureKind::Cancelled, "Download cancelled")
    }

    pub(super) fn too_large(max_bytes: u64) -> Self {
        Self::new(
            FailureKind::TooLarge,
            format!("Download exceeds max_bytes ({max_bytes} bytes)"),
        )
    }
}

pub(super) enum ProbeMode {
    Sequential(Response),
    Parallel,
    Empty,
}

pub(super) struct ProbeResult {
    pub mode: ProbeMode,
    pub final_url: Url,
    pub total_size: Option<u64>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
    pub validator: Option<HeaderValue>,
    pub range_supported: bool,
}

pub(super) struct ParallelDownloadOptions {
    pub url: Url,
    pub proxy_url: Option<String>,
    pub total_size: u64,
    pub requested_connections: usize,
    pub validator: Option<HeaderValue>,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedContentRange {
    start: Option<u64>,
    end: Option<u64>,
    total: Option<u64>,
}

pub(super) async fn probe_server(
    url: Url,
    proxy_url: Option<&str>,
    max_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<ProbeResult, DownloadFailure> {
    let mut last_error = None;
    for attempt in 0..RETRY_ATTEMPTS {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(RANGE, HeaderValue::from_static("bytes=0-0"));
        let fetched = match send_get(url.clone(), proxy_url, headers, cancellation).await {
            Ok(fetched) => fetched,
            Err(error) if retryable(&error) && attempt + 1 < RETRY_ATTEMPTS => {
                last_error = Some(error);
                sleep_before_retry(attempt, None, cancellation).await?;
                continue;
            }
            Err(error) => return Err(error),
        };

        let status = fetched.response.status();
        if rate_limited(&fetched.response) {
            let retry_after_ms = retry_after_ms(fetched.response.headers());
            let error = DownloadFailure::new(
                FailureKind::RateLimited(retry_after_ms),
                format!("Download server returned HTTP {status}"),
            );
            if attempt + 1 < RETRY_ATTEMPTS {
                last_error = Some(error);
                sleep_before_retry(attempt, retry_after_ms, cancellation).await?;
                continue;
            }
            return Err(error);
        }
        if status.is_server_error() {
            let error = DownloadFailure::new(
                FailureKind::Network,
                format!("Download server returned HTTP {status}"),
            );
            if attempt + 1 < RETRY_ATTEMPTS {
                last_error = Some(error);
                sleep_before_retry(attempt, None, cancellation).await?;
                continue;
            }
            return Err(error);
        }

        return probe_response(fetched, proxy_url, max_bytes, cancellation).await;
    }

    Err(last_error
        .unwrap_or_else(|| DownloadFailure::new(FailureKind::Network, "Download probe failed")))
}

async fn probe_response(
    fetched: SafeHttpResponse,
    proxy_url: Option<&str>,
    max_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<ProbeResult, DownloadFailure> {
    let status = fetched.response.status();
    if matches!(
        status,
        StatusCode::FORBIDDEN | StatusCode::METHOD_NOT_ALLOWED
    ) {
        let fetched = request_full_response(fetched.final_url, proxy_url, cancellation).await?;
        return sequential_probe_result(fetched, max_bytes);
    }
    if status == StatusCode::RANGE_NOT_SATISFIABLE
        && parse_unsatisfied_total(fetched.response.headers()) == Some(0)
    {
        return Ok(ProbeResult {
            mode: ProbeMode::Empty,
            final_url: fetched.final_url,
            total_size: Some(0),
            content_type: header_string(fetched.response.headers(), CONTENT_TYPE),
            content_disposition: header_string(fetched.response.headers(), CONTENT_DISPOSITION),
            validator: response_validator(fetched.response.headers()),
            range_supported: true,
        });
    }
    if status == StatusCode::PARTIAL_CONTENT {
        let parsed = parse_content_range_header(fetched.response.headers()).ok_or_else(|| {
            DownloadFailure::new(
                FailureKind::Protocol,
                "Range probe returned an invalid Content-Range header",
            )
        })?;
        let total = match (parsed.start, parsed.end, parsed.total) {
            (Some(0), Some(0), Some(total)) if total > 0 => total,
            _ => {
                return Err(DownloadFailure::new(
                    FailureKind::Protocol,
                    "Range probe did not describe bytes 0-0 of a known resource",
                ))
            }
        };
        enforce_size(total, max_bytes)?;
        if content_length(fetched.response.headers()).is_some_and(|length| length != 1) {
            return Err(DownloadFailure::new(
                FailureKind::Protocol,
                "Range probe returned an unexpected Content-Length",
            ));
        }
        let content_type = header_string(fetched.response.headers(), CONTENT_TYPE);
        let content_disposition = header_string(fetched.response.headers(), CONTENT_DISPOSITION);
        let validator = response_validator(fetched.response.headers());
        consume_exact_body(fetched.response, 1, cancellation).await?;
        return Ok(ProbeResult {
            mode: ProbeMode::Parallel,
            final_url: fetched.final_url,
            total_size: Some(total),
            content_type,
            content_disposition,
            validator,
            range_supported: true,
        });
    }
    if status.is_success() {
        return sequential_probe_result(fetched, max_bytes);
    }
    Err(DownloadFailure::new(
        FailureKind::Network,
        format!("Download server returned HTTP {status}"),
    ))
}

fn sequential_probe_result(
    fetched: SafeHttpResponse,
    max_bytes: u64,
) -> Result<ProbeResult, DownloadFailure> {
    let total_size = content_length(fetched.response.headers());
    if let Some(total) = total_size {
        enforce_size(total, max_bytes)?;
    }
    Ok(ProbeResult {
        content_type: header_string(fetched.response.headers(), CONTENT_TYPE),
        content_disposition: header_string(fetched.response.headers(), CONTENT_DISPOSITION),
        validator: response_validator(fetched.response.headers()),
        mode: ProbeMode::Sequential(fetched.response),
        final_url: fetched.final_url,
        total_size,
        range_supported: false,
    })
}

pub(super) async fn download_sequential(
    mut file: tokio::fs::File,
    response: Option<Response>,
    url: Url,
    proxy_url: Option<&str>,
    max_bytes: u64,
    expected_size: Option<u64>,
    cancellation: &CancellationToken,
) -> Result<(tokio::fs::File, u64), DownloadFailure> {
    file.set_len(0).await.map_err(|error| {
        DownloadFailure::new(
            FailureKind::Io,
            format!("Failed to truncate temporary download: {error}"),
        )
    })?;
    file.seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| {
            DownloadFailure::new(
                FailureKind::Io,
                format!("Failed to seek temporary download: {error}"),
            )
        })?;

    let response = match response {
        Some(response) => response,
        None => {
            request_full_response(url, proxy_url, cancellation)
                .await?
                .response
        }
    };
    let status = response.status();
    if !status.is_success() || status == StatusCode::PARTIAL_CONTENT {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            format!("Sequential download returned HTTP {status}"),
        ));
    }
    if let Some(length) = content_length(response.headers()) {
        enforce_size(length, max_bytes)?;
        if expected_size.is_some_and(|expected| expected != length) {
            return Err(DownloadFailure::new(
                FailureKind::Protocol,
                "Download size changed after the server probe",
            ));
        }
    }

    let mut stream = response.bytes_stream();
    let mut written = 0_u64;
    loop {
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err(DownloadFailure::cancelled()),
            next = stream.next() => next,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| {
            DownloadFailure::new(
                FailureKind::Network,
                format!("Failed to read download response: {}", error.without_url()),
            )
        })?;
        written = written
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| DownloadFailure::too_large(max_bytes))?;
        enforce_size(written, max_bytes)?;
        file.write_all(&chunk).await.map_err(|error| {
            DownloadFailure::new(
                FailureKind::Io,
                format!("Failed to write temporary download: {error}"),
            )
        })?;
    }
    if expected_size.is_some_and(|expected| expected != written) {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            format!(
                "Download ended after {written} bytes; expected {} bytes",
                expected_size.unwrap_or_default()
            ),
        ));
    }
    Ok((file, written))
}

pub(super) async fn download_parallel(
    file: tokio::fs::File,
    options: ParallelDownloadOptions,
    cancellation: &CancellationToken,
) -> Result<(tokio::fs::File, usize), DownloadFailure> {
    let ParallelDownloadOptions {
        url,
        proxy_url,
        total_size,
        requested_connections,
        validator,
        max_bytes,
    } = options;
    enforce_size(total_size, max_bytes)?;
    file.set_len(total_size).await.map_err(|error| {
        DownloadFailure::new(
            FailureKind::Io,
            format!("Failed to preallocate temporary download: {error}"),
        )
    })?;
    let ranges = split_ranges(total_size, requested_connections);
    let connection_count = ranges.len();
    let file = Arc::new(Mutex::new(file));
    let worker_cancellation = cancellation.child_token();
    let mut tasks = tokio::task::JoinSet::new();

    for range in ranges {
        let file = Arc::clone(&file);
        let url = url.clone();
        let proxy_url = proxy_url.clone();
        let validator = validator.clone();
        let cancellation = worker_cancellation.clone();
        tasks.spawn(async move {
            download_range(
                file,
                url,
                proxy_url.as_deref(),
                range,
                total_size,
                validator,
                cancellation,
            )
            .await
        });
    }

    let mut first_error = None;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                    worker_cancellation.cancel();
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(DownloadFailure::new(
                        FailureKind::Io,
                        format!("Download worker failed: {error}"),
                    ));
                    worker_cancellation.cancel();
                }
            }
        }
    }
    if cancellation.is_cancelled() {
        return Err(DownloadFailure::cancelled());
    }
    if let Some(error) = first_error {
        return Err(error);
    }

    let mutex = Arc::try_unwrap(file).map_err(|_| {
        DownloadFailure::new(
            FailureKind::Io,
            "Download workers did not release the output file",
        )
    })?;
    Ok((mutex.into_inner(), connection_count))
}

async fn download_range(
    file: Arc<Mutex<tokio::fs::File>>,
    url: Url,
    proxy_url: Option<&str>,
    range: ByteRange,
    total_size: u64,
    validator: Option<HeaderValue>,
    cancellation: CancellationToken,
) -> Result<(), DownloadFailure> {
    let mut last_error = None;
    for attempt in 0..RETRY_ATTEMPTS {
        match download_range_once(
            Arc::clone(&file),
            url.clone(),
            proxy_url,
            range,
            total_size,
            validator.clone(),
            &cancellation,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) if retryable(&error) && attempt + 1 < RETRY_ATTEMPTS => {
                let retry_after = match error.kind {
                    FailureKind::RateLimited(value) => value,
                    _ => None,
                };
                last_error = Some(error);
                sleep_before_retry(attempt, retry_after, &cancellation).await?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| DownloadFailure::new(FailureKind::Network, "Range download failed")))
}

async fn download_range_once(
    file: Arc<Mutex<tokio::fs::File>>,
    url: Url,
    proxy_url: Option<&str>,
    range: ByteRange,
    total_size: u64,
    validator: Option<HeaderValue>,
    cancellation: &CancellationToken,
) -> Result<(), DownloadFailure> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(
        RANGE,
        HeaderValue::from_str(&format!("bytes={}-{}", range.start, range.end))
            .map_err(|_| DownloadFailure::new(FailureKind::Protocol, "Invalid byte range"))?,
    );
    if let Some(validator) = validator {
        headers.insert(IF_RANGE, validator);
    }
    let requested_url = url.clone();
    let fetched = send_get(url, proxy_url, headers, cancellation).await?;
    if !same_origin(&requested_url, &fetched.final_url) {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            "Range request redirected across origins after the server probe",
        ));
    }
    let response = fetched.response;
    let status = response.status();
    if rate_limited(&response) {
        let retry_after = retry_after_ms(response.headers());
        return Err(DownloadFailure::new(
            FailureKind::RateLimited(retry_after),
            format!("Range request returned HTTP {status}"),
        ));
    }
    if status.is_server_error() {
        return Err(DownloadFailure::new(
            FailureKind::Network,
            format!("Range request returned HTTP {status}"),
        ));
    }
    if status != StatusCode::PARTIAL_CONTENT {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            format!("Range request expected HTTP 206, received {status}"),
        ));
    }
    let parsed = parse_content_range_header(response.headers()).ok_or_else(|| {
        DownloadFailure::new(
            FailureKind::Protocol,
            "Range response has an invalid Content-Range header",
        )
    })?;
    if parsed.start != Some(range.start)
        || parsed.end != Some(range.end)
        || parsed.total != Some(total_size)
    {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            "Range response does not match the requested byte interval",
        ));
    }
    if content_length(response.headers()).is_some_and(|length| length != range.len()) {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            "Range response Content-Length does not match its interval",
        ));
    }

    let mut stream = response.bytes_stream();
    let mut received = 0_u64;
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(DownloadFailure::cancelled()),
        next = stream.next() => next,
    } {
        let chunk = chunk.map_err(|error| {
            DownloadFailure::new(
                FailureKind::Network,
                format!("Failed to read range response: {}", error.without_url()),
            )
        })?;
        let next_received = received.checked_add(chunk.len() as u64).ok_or_else(|| {
            DownloadFailure::new(FailureKind::Protocol, "Range response is too large")
        })?;
        if next_received > range.len() {
            return Err(DownloadFailure::new(
                FailureKind::Protocol,
                "Range response contained more bytes than requested",
            ));
        }
        let mut output = file.lock().await;
        output
            .seek(std::io::SeekFrom::Start(range.start + received))
            .await
            .map_err(|error| {
                DownloadFailure::new(
                    FailureKind::Io,
                    format!("Failed to seek temporary download: {error}"),
                )
            })?;
        output.write_all(&chunk).await.map_err(|error| {
            DownloadFailure::new(
                FailureKind::Io,
                format!("Failed to write range response: {error}"),
            )
        })?;
        received = next_received;
    }
    if received != range.len() {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            format!(
                "Range response ended after {received} bytes; expected {} bytes",
                range.len()
            ),
        ));
    }
    Ok(())
}

async fn request_full_response(
    url: Url,
    proxy_url: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<SafeHttpResponse, DownloadFailure> {
    let mut last_error = None;
    for attempt in 0..RETRY_ATTEMPTS {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let fetched = match send_get(url.clone(), proxy_url, headers, cancellation).await {
            Ok(fetched) => fetched,
            Err(error) if retryable(&error) && attempt + 1 < RETRY_ATTEMPTS => {
                last_error = Some(error);
                sleep_before_retry(attempt, None, cancellation).await?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let status = fetched.response.status();
        if rate_limited(&fetched.response) {
            let retry_after = retry_after_ms(fetched.response.headers());
            let error = DownloadFailure::new(
                FailureKind::RateLimited(retry_after),
                format!("Download server returned HTTP {status}"),
            );
            if attempt + 1 < RETRY_ATTEMPTS {
                last_error = Some(error);
                sleep_before_retry(attempt, retry_after, cancellation).await?;
                continue;
            }
            return Err(error);
        }
        if status.is_server_error() {
            let error = DownloadFailure::new(
                FailureKind::Network,
                format!("Download server returned HTTP {status}"),
            );
            if attempt + 1 < RETRY_ATTEMPTS {
                last_error = Some(error);
                sleep_before_retry(attempt, None, cancellation).await?;
                continue;
            }
            return Err(error);
        }
        if !status.is_success() || status == StatusCode::PARTIAL_CONTENT {
            return Err(DownloadFailure::new(
                FailureKind::Protocol,
                format!("Download server returned HTTP {status}"),
            ));
        }
        return Ok(fetched);
    }
    Err(last_error
        .unwrap_or_else(|| DownloadFailure::new(FailureKind::Network, "Download request failed")))
}

async fn send_get(
    url: Url,
    proxy_url: Option<&str>,
    headers: HeaderMap,
    cancellation: &CancellationToken,
) -> Result<SafeHttpResponse, DownloadFailure> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(DownloadFailure::cancelled()),
        result = get_with_redirects(
            url,
            proxy_url,
            headers,
            MAX_REDIRECTS,
            RedirectQueryPolicy::Preserve,
        ) => result.map_err(|error| {
            DownloadFailure::new(FailureKind::Network, error.to_string())
        }),
    }
}

async fn consume_exact_body(
    response: Response,
    expected: u64,
    cancellation: &CancellationToken,
) -> Result<(), DownloadFailure> {
    let mut stream = response.bytes_stream();
    let mut received = 0_u64;
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(DownloadFailure::cancelled()),
        next = stream.next() => next,
    } {
        let chunk = chunk.map_err(|error| {
            DownloadFailure::new(
                FailureKind::Network,
                format!("Failed to read probe response: {}", error.without_url()),
            )
        })?;
        received = received.saturating_add(chunk.len() as u64);
        if received > expected {
            return Err(DownloadFailure::new(
                FailureKind::Protocol,
                "Range probe returned too many bytes",
            ));
        }
    }
    if received != expected {
        return Err(DownloadFailure::new(
            FailureKind::Protocol,
            "Range probe returned an incomplete body",
        ));
    }
    Ok(())
}

pub(super) fn connection_count(total_size: u64, requested: Option<usize>) -> usize {
    if total_size == 0 {
        return 1;
    }
    let max_by_chunk = usize::try_from((total_size / MIN_CHUNK_SIZE).max(1)).unwrap_or(usize::MAX);
    let adaptive = || {
        let size_mib = total_size as f64 / (1024.0 * 1024.0);
        size_mib.sqrt().round().max(1.0) as usize
    };
    requested
        .unwrap_or_else(adaptive)
        .clamp(1, MAX_CONNECTIONS)
        .min(max_by_chunk)
        .max(1)
}

pub(super) fn split_ranges(total_size: u64, connections: usize) -> Vec<ByteRange> {
    if total_size == 0 {
        return Vec::new();
    }
    let connections = connections.max(1) as u64;
    let raw_chunk = total_size.div_ceil(connections);
    let chunk_size = raw_chunk.div_ceil(RANGE_ALIGNMENT) * RANGE_ALIGNMENT;
    let mut ranges = Vec::new();
    let mut start = 0_u64;
    while start < total_size {
        let end = start
            .saturating_add(chunk_size)
            .min(total_size)
            .saturating_sub(1);
        ranges.push(ByteRange { start, end });
        start = end.saturating_add(1);
    }
    ranges
}

fn parse_content_range_header(headers: &HeaderMap) -> Option<ParsedContentRange> {
    let value = headers.get(CONTENT_RANGE)?.to_str().ok()?.trim();
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let total = (total != "*").then(|| total.parse::<u64>().ok()).flatten();
    if range == "*" {
        return Some(ParsedContentRange {
            start: None,
            end: None,
            total,
        });
    }
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    (start <= end).then_some(ParsedContentRange {
        start: Some(start),
        end: Some(end),
        total,
    })
}

fn parse_unsatisfied_total(headers: &HeaderMap) -> Option<u64> {
    let parsed = parse_content_range_header(headers)?;
    (parsed.start.is_none() && parsed.end.is_none()).then_some(parsed.total?)
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn response_validator(headers: &HeaderMap) -> Option<HeaderValue> {
    let etag = headers.get(ETAG).filter(|value| {
        value
            .to_str()
            .ok()
            .is_some_and(|value| !value.trim_start().starts_with("W/"))
    });
    etag.or_else(|| headers.get(LAST_MODIFIED)).cloned()
}

fn header_string(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn enforce_size(size: u64, max_bytes: u64) -> Result<(), DownloadFailure> {
    if size > max_bytes {
        Err(DownloadFailure::too_large(max_bytes))
    } else {
        Ok(())
    }
}

fn rate_limited(response: &Response) -> bool {
    response.status() == StatusCode::TOO_MANY_REQUESTS
        || (response.status() == StatusCode::SERVICE_UNAVAILABLE
            && response.headers().contains_key(RETRY_AFTER))
}

fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.min(30).saturating_mul(1_000));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let remaining = date.with_timezone(&chrono::Utc) - chrono::Utc::now();
    let milliseconds = remaining.num_milliseconds().max(0) as u64;
    Some(milliseconds.min(30_000))
}

fn retryable(error: &DownloadFailure) -> bool {
    matches!(
        error.kind,
        FailureKind::RateLimited(_) | FailureKind::Network | FailureKind::Protocol
    )
}

async fn sleep_before_retry(
    attempt: usize,
    retry_after_ms: Option<u64>,
    cancellation: &CancellationToken,
) -> Result<(), DownloadFailure> {
    let exponential = RETRY_BASE_DELAY_MS.saturating_mul(1_u64 << attempt.min(8));
    let delay = Duration::from_millis(retry_after_ms.unwrap_or(exponential).min(30_000));
    tokio::select! {
        _ = cancellation.cancelled() => Err(DownloadFailure::cancelled()),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_connections_are_bounded_by_size_and_policy() {
        assert_eq!(connection_count(1, None), 1);
        assert_eq!(connection_count(3 * 1024 * 1024, Some(4)), 1);
        assert_eq!(connection_count(8 * 1024 * 1024, Some(4)), 4);
        assert_eq!(connection_count(100 * 1024 * 1024, None), 4);
    }

    #[test]
    fn ranges_cover_the_file_without_gaps_or_overlap() {
        let total = 9 * 1024 * 1024 + 17;
        let ranges = split_ranges(total, 4);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, total - 1);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end + 1, pair[1].start);
        }
        assert_eq!(ranges.iter().map(|range| range.len()).sum::<u64>(), total);
    }

    #[test]
    fn content_range_parser_handles_satisfied_and_empty_forms() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, "bytes 10-19/100".parse().unwrap());
        assert_eq!(
            parse_content_range_header(&headers),
            Some(ParsedContentRange {
                start: Some(10),
                end: Some(19),
                total: Some(100),
            })
        );
        headers.insert(CONTENT_RANGE, "bytes */0".parse().unwrap());
        assert_eq!(parse_unsatisfied_total(&headers), Some(0));
    }
}
