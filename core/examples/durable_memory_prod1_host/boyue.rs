//! Real OpenAI-compatible embedding provider (Boyue gateway).
//!
//! Credentials arrive only through the environment and are never echoed into a
//! descriptor, an error, or a report. Provider response bodies are parsed for
//! vectors and token usage only; no remote text is retained.

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Public provider name recorded in the semantic binding.
pub const PROVIDER_ID: &str = "boyue.openai-compatible";
/// Default embedding model for the qualification pack.
pub const DEFAULT_MODEL: &str = "text-embedding-3-small";
/// Published `text-embedding-3-small` list price, in USD per million tokens.
pub const DEFAULT_USD_PER_MILLION_TOKENS: f64 = 0.02;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Endpoint and model identity, with the credential kept out of the struct.
#[derive(Clone, Debug)]
pub struct BoyueEndpoint {
    pub base_url: String,
    pub model: String,
    pub usd_per_million_tokens: f64,
}

impl BoyueEndpoint {
    /// Host and port only, so no query-string credential can leak into a report.
    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.base_url)
            .ok()
            .and_then(|url| {
                url.host_str().map(|host| match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_string(),
                })
            })
            .unwrap_or_else(|| "unparsed".to_string())
    }

    /// Stable opaque identity for the exact endpoint that served a generation.
    pub fn endpoint_digest(&self) -> String {
        format!("sha256:{:x}", Sha256::digest(self.base_url.as_bytes()))
    }

    fn embeddings_url(&self) -> String {
        format!("{}/embeddings", self.base_url.trim_end_matches('/'))
    }
}

/// Latency, token, and failure distributions for one provider generation.
#[derive(Clone, Debug, Default)]
pub struct ProviderTelemetry {
    pub requests: u64,
    pub inputs: u64,
    pub failures: u64,
    pub prompt_tokens: u64,
    pub total_tokens: u64,
    pub latencies_ms: Vec<f64>,
    pub vector_norms: Vec<f64>,
}

impl ProviderTelemetry {
    fn observe_success(&mut self, inputs: usize, latency: Duration, usage: Option<&Usage>) {
        self.requests = self.requests.saturating_add(1);
        self.inputs = self.inputs.saturating_add(inputs as u64);
        self.latencies_ms.push(latency.as_secs_f64() * 1_000.0);
        if let Some(usage) = usage {
            self.prompt_tokens = self.prompt_tokens.saturating_add(usage.prompt_tokens);
            self.total_tokens = self.total_tokens.saturating_add(usage.total_tokens);
        }
    }
}

/// OpenAI-compatible embedding adapter over one Boyue endpoint.
pub struct BoyueEmbeddingProvider {
    client: reqwest::Client,
    endpoint: BoyueEndpoint,
    api_key: String,
    dimension: usize,
    telemetry: Mutex<ProviderTelemetry>,
}

impl std::fmt::Debug for BoyueEmbeddingProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoyueEmbeddingProvider")
            .field("provider", &PROVIDER_ID)
            .field("model", &self.endpoint.model)
            .field("host", &self.endpoint.host())
            .field("dimension", &self.dimension)
            .finish_non_exhaustive()
    }
}

impl BoyueEmbeddingProvider {
    /// Probe the endpoint once to learn the served dimension before binding.
    ///
    /// Probing is what lets the semantic binding record an exact output shape
    /// instead of trusting a configured constant.
    pub async fn probe(
        endpoint: BoyueEndpoint,
        api_key: String,
    ) -> Result<Self, ProviderSetupError> {
        if api_key.trim().is_empty() {
            return Err(ProviderSetupError::MissingCredential);
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ProviderSetupError::ClientBuild)?;
        let provider = Self {
            client,
            endpoint,
            api_key,
            dimension: 0,
            telemetry: Mutex::new(ProviderTelemetry::default()),
        };
        let response = provider
            .request(&["a3s dm-prod1 endpoint probe".to_string()])
            .await
            .map_err(ProviderSetupError::Probe)?;
        let dimension = response
            .vectors
            .first()
            .map(Vec::len)
            .filter(|dimension| *dimension > 0)
            .ok_or(ProviderSetupError::EmptyProbe)?;
        Ok(Self {
            dimension,
            ..provider
        })
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    pub fn telemetry(&self) -> ProviderTelemetry {
        self.telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Estimated billed cost for every request this provider has served.
    pub fn estimated_usd(&self) -> f64 {
        let telemetry = self.telemetry();
        (telemetry.total_tokens as f64) * self.endpoint.usd_per_million_tokens / 1_000_000.0
    }

    async fn request(&self, texts: &[String]) -> Result<EmbeddingsPayload, EmbeddingProviderError> {
        let started = Instant::now();
        let response = self
            .client
            .post(self.endpoint.embeddings_url())
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.endpoint.model,
                "input": texts,
            }))
            .send()
            .await
            .map_err(classify_transport)?;
        let status = response.status();
        if !status.is_success() {
            self.observe_failure();
            return Err(classify_status(status, &response));
        }
        let body = response.bytes().await.map_err(classify_transport)?;
        let latency = started.elapsed();
        let parsed: EmbeddingsResponse = serde_json::from_slice(&body).map_err(|_| {
            self.observe_failure();
            EmbeddingProviderError::Other
        })?;
        if parsed.data.len() != texts.len() {
            self.observe_failure();
            return Err(EmbeddingProviderError::Other);
        }
        let mut ordered = vec![Vec::new(); texts.len()];
        for item in parsed.data {
            if item.index >= ordered.len() || !ordered[item.index].is_empty() {
                self.observe_failure();
                return Err(EmbeddingProviderError::Other);
            }
            ordered[item.index] = item.embedding;
        }
        {
            let mut telemetry = self
                .telemetry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            telemetry.observe_success(texts.len(), latency, parsed.usage.as_ref());
            for vector in &ordered {
                telemetry.vector_norms.push(l2_norm(vector));
            }
        }
        Ok(EmbeddingsPayload { vectors: ordered })
    }

    fn observe_failure(&self) {
        let mut telemetry = self
            .telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        telemetry.failures = telemetry.failures.saturating_add(1);
    }
}

#[async_trait]
impl EmbeddingProvider for BoyueEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        // `revision` carries the endpoint identity, not the credential, so a
        // persisted binding still fails closed when the serving host changes.
        EmbeddingProviderDescriptor::new(PROVIDER_ID, &self.endpoint.model, self.dimension)
            .with_revision(self.endpoint.endpoint_digest())
            .with_normalization(EmbeddingNormalization::None)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let texts: Vec<String> = request
            .inputs()
            .iter()
            .map(|input| input.text().to_string())
            .collect();
        let payload = tokio::select! {
            result = self.request(&texts) => result?,
            _ = cancellation.cancelled() => return Err(EmbeddingProviderError::Cancelled),
        };
        let vectors = request
            .inputs()
            .iter()
            .zip(payload.vectors)
            .map(|(input, values)| EmbeddingVector::new(input.id(), values))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

struct EmbeddingsPayload {
    vectors: Vec<Vec<f32>>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    #[serde(default)]
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// Setup failures that keep the harness from constructing a real generation.
#[derive(Debug, thiserror::Error)]
pub enum ProviderSetupError {
    #[error("BOYUE_API_KEY is empty; export it from scripts/harbor/.env")]
    MissingCredential,
    #[error("could not build the embedding HTTP client")]
    ClientBuild,
    #[error("embedding endpoint probe failed: {0}")]
    Probe(#[source] EmbeddingProviderError),
    #[error("embedding endpoint probe returned no vector")]
    EmptyProbe,
}

fn classify_transport(error: reqwest::Error) -> EmbeddingProviderError {
    if error.is_timeout() {
        EmbeddingProviderError::Timeout
    } else if error.is_connect() {
        EmbeddingProviderError::Unavailable { retry_after: None }
    } else {
        EmbeddingProviderError::Other
    }
}

fn classify_status(
    status: reqwest::StatusCode,
    response: &reqwest::Response,
) -> EmbeddingProviderError {
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    match status.as_u16() {
        401 | 403 => EmbeddingProviderError::Authentication,
        408 => EmbeddingProviderError::Timeout,
        429 => EmbeddingProviderError::RateLimited { retry_after },
        400 | 404 | 422 => EmbeddingProviderError::InvalidRequest,
        500..=599 => EmbeddingProviderError::Unavailable { retry_after },
        _ => EmbeddingProviderError::Other,
    }
}

fn l2_norm(vector: &[f32]) -> f64 {
    vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt()
}
