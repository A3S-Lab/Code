use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    HttpClient, HttpResponse, OpenAiClient, RetryConfig, StreamingHttpResponse,
};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct AlwaysRateLimitedHttp {
    calls: AtomicUsize,
}

impl AlwaysRateLimitedHttp {
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HttpClient for AlwaysRateLimitedHttp {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel_token: CancellationToken,
    ) -> Result<HttpResponse> {
        anyhow::bail!("non-streaming transport is not part of this fixture")
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(StreamingHttpResponse {
            status: 429,
            retry_after: Some("0".to_string()),
            byte_stream: Box::pin(futures::stream::empty::<Result<bytes::Bytes>>()),
            error_body: "rate limited fixture".to_string(),
        })
    }
}

fn fixture_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/gpt".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "gpt".into(),
                name: "Fixture GPT".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn exhausted_provider_retry_is_not_replayed_by_agent_boundary() {
    let http = Arc::new(AlwaysRateLimitedHttp {
        calls: AtomicUsize::new(0),
    });
    let provider = OpenAiClient::new("fixture-key".into(), "fixture-model".into())
        .with_http_client(http.clone())
        .with_retry_config(RetryConfig {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            ..Default::default()
        });

    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(fixture_config()).await.unwrap();
    let session = agent
        .session_builder(workspace.path().display().to_string())
        .options(
            SessionOptions::new()
                .with_llm_client(Arc::new(provider))
                .with_planning_mode(PlanningMode::Disabled)
                .with_circuit_breaker(3),
        )
        .build()
        .await
        .unwrap();

    let (mut events, worker) = session.stream("complete the task", None).await.unwrap();
    let mut errors = Vec::new();
    while let Some(event) = events.recv().await {
        if let AgentEvent::Error { message } = event {
            errors.push(message);
        }
    }
    worker.await.unwrap();
    session.close().await;

    assert_eq!(
        http.call_count(),
        2,
        "one inner retry budget must not be replayed by the outer circuit breaker"
    );
    assert!(errors
        .iter()
        .any(|message| { message.contains("LLM API request failed after 2 attempts") }));
}
