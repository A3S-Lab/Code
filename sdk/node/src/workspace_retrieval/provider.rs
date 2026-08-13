use super::*;

const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 30_000;
const MAX_PROVIDER_TIMEOUT_MS: u64 = 300_000;
const CALLBACK_WRAPPER_FACTORY: &str = r#"
(function a3sCreateSafeAsyncEmbeddingCallback(callback) {
  const active = new Map();
  const cancelledBeforeStart = new Set();

  function failure(error) {
    let message = "JavaScript embedding callback failed";
    try {
      if (error !== null && error !== undefined) {
        message = typeof error.message === "string" ? error.message : String(error);
      }
    } catch (_) {}
    return { __a3sEmbeddingCallbackV1: true, ok: false, error: message };
  }

  return {
    run: async function a3sSafeAsyncEmbeddingCallback(request) {
      const controller = new AbortController();
      active.set(request.requestId, controller);
      if (cancelledBeforeStart.delete(request.requestId)) {
        controller.abort();
      }
      try {
        const value = await callback.call(this, {
          inputs: request.inputs,
          textBytes: request.textBytes,
          signal: controller.signal,
        });
        return { __a3sEmbeddingCallbackV1: true, ok: true, value };
      } catch (error) {
        return failure(error);
      } finally {
        active.delete(request.requestId);
      }
    },
    abort: function a3sAbortEmbeddingCallback(requestId) {
      const controller = active.get(requestId);
      if (controller !== undefined) {
        controller.abort();
        active.delete(requestId);
      } else {
        cancelledBeforeStart.add(requestId);
        if (cancelledBeforeStart.size > 1024) {
          cancelledBeforeStart.delete(cancelledBeforeStart.values().next().value);
        }
      }
    }
  };
})
"#;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeEmbeddingInput {
    id: String,
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeEmbeddingBatchRequest {
    request_id: String,
    inputs: Vec<NodeEmbeddingInput>,
    text_bytes: usize,
}

impl NodeEmbeddingBatchRequest {
    fn new(request_id: String, request: &EmbeddingBatchRequest) -> Self {
        Self {
            request_id,
            inputs: request
                .inputs()
                .iter()
                .map(|input| NodeEmbeddingInput {
                    id: input.id().to_owned(),
                    text: input.text().to_owned(),
                })
                .collect(),
            text_bytes: request.text_bytes(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeEmbeddingVector {
    id: String,
    values: Vec<f32>,
}

#[derive(Deserialize)]
struct NodeEmbeddingSuccess {
    vectors: Vec<NodeEmbeddingVector>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeEmbeddingFailure {
    kind: String,
    retry_after_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NodeEmbeddingValue {
    Success(NodeEmbeddingSuccess),
    Failure(NodeEmbeddingFailure),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeEmbeddingEnvelope {
    #[serde(rename = "__a3sEmbeddingCallbackV1")]
    marker: bool,
    ok: bool,
    value: Option<NodeEmbeddingValue>,
    error: Option<String>,
}

pub(super) struct NodeEmbeddingProvider {
    descriptor: EmbeddingProviderDescriptor,
    timeout: Duration,
    callback: ThreadsafeFunction<NodeEmbeddingBatchRequest, ErrorStrategy::Fatal>,
    abort: ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    next_request_id: AtomicU64,
}

struct NodeEmbeddingAbortGuard<'a> {
    request_id: String,
    abort: &'a ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    armed: bool,
}

impl NodeEmbeddingAbortGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for NodeEmbeddingAbortGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.abort.call(
                self.request_id.clone(),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        }
    }
}

#[async_trait]
impl EmbeddingProvider for NodeEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let mut abort_guard = NodeEmbeddingAbortGuard {
            request_id: request_id.clone(),
            abort: &self.abort,
            armed: true,
        };
        let callback = async {
            self.callback
                .call_async::<Promise<serde_json::Value>>(NodeEmbeddingBatchRequest::new(
                    request_id, &request,
                ))
                .await?
                .await
        };
        let value = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(EmbeddingProviderError::Cancelled),
            value = tokio::time::timeout(self.timeout, callback) => match value {
                Ok(Ok(value)) => value,
                Ok(Err(_)) => return Err(EmbeddingProviderError::Other),
                Err(_) => return Err(EmbeddingProviderError::Timeout),
            },
        };
        abort_guard.disarm();
        let envelope: NodeEmbeddingEnvelope =
            serde_json::from_value(value).map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        if !envelope.marker {
            return Err(EmbeddingProviderError::InvalidRequest);
        }
        if !envelope.ok {
            let _ = envelope.error;
            return Err(EmbeddingProviderError::Other);
        }
        match envelope
            .value
            .ok_or(EmbeddingProviderError::InvalidRequest)?
        {
            NodeEmbeddingValue::Success(response) => Ok(EmbeddingBatchResponse::new(
                self.descriptor.clone(),
                response
                    .vectors
                    .into_iter()
                    .map(|vector| EmbeddingVector::new(vector.id, vector.values))
                    .collect(),
            )),
            NodeEmbeddingValue::Failure(failure) => Err(provider_failure(failure)),
        }
    }
}

fn provider_failure(failure: NodeEmbeddingFailure) -> EmbeddingProviderError {
    let retry_after = failure.retry_after_ms.map(Duration::from_millis);
    match failure.kind.as_str() {
        "cancelled" => EmbeddingProviderError::Cancelled,
        "timeout" => EmbeddingProviderError::Timeout,
        "rate_limited" => EmbeddingProviderError::RateLimited { retry_after },
        "unavailable" => EmbeddingProviderError::Unavailable { retry_after },
        "authentication" => EmbeddingProviderError::Authentication,
        "invalid_request" => EmbeddingProviderError::InvalidRequest,
        _ => EmbeddingProviderError::Other,
    }
}

/// Host-injected asynchronous embedding provider for session-bound retrieval.
#[napi]
pub struct CallbackEmbeddingProvider {
    pub(super) instance_id: String,
    pub(super) inner: Arc<NodeEmbeddingProvider>,
}

#[napi]
impl CallbackEmbeddingProvider {
    #[napi(
        constructor,
        ts_args_type = "descriptor: EmbeddingProviderDescriptorObject, embed: (request: EmbeddingBatchRequest) => Promise<EmbeddingBatchResponse | EmbeddingBatchFailure>, timeoutMs?: number | null"
    )]
    pub fn new(
        env: Env,
        descriptor: EmbeddingProviderDescriptorObject,
        embed: napi::JsFunction,
        timeout_ms: Option<f64>,
    ) -> napi::Result<Self> {
        let descriptor = descriptor.to_core()?;
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_PROVIDER_TIMEOUT_MS as f64);
        let timeout_ms = js_optional_usize(
            Some(timeout_ms),
            "CallbackEmbeddingProvider.timeoutMs",
            DEFAULT_PROVIDER_TIMEOUT_MS as usize,
        )?;
        if timeout_ms == 0 || timeout_ms > MAX_PROVIDER_TIMEOUT_MS as usize {
            return Err(napi::Error::from_reason(format!(
                "CallbackEmbeddingProvider.timeoutMs must be from 1 to {MAX_PROVIDER_TIMEOUT_MS}"
            )));
        }
        let factory: napi::JsFunction = env.run_script(CALLBACK_WRAPPER_FACTORY)?;
        let wrapped = factory.call(None, &[embed])?;
        let wrapped = napi::JsObject::try_from(wrapped).map_err(|_| {
            napi::Error::from_reason("failed to construct embedding callback bridge")
        })?;
        let run = wrapped.get_named_property::<napi::JsFunction>("run")?;
        let abort = wrapped.get_named_property::<napi::JsFunction>("abort")?;
        let mut callback: ThreadsafeFunction<NodeEmbeddingBatchRequest, ErrorStrategy::Fatal> = run
            .create_threadsafe_function(
                0,
                |ctx: ThreadSafeCallContext<NodeEmbeddingBatchRequest>| {
                    let value = ctx.env.to_js_value(&ctx.value)?;
                    Ok(vec![value])
                },
            )?;
        let mut abort: ThreadsafeFunction<String, ErrorStrategy::Fatal> = abort
            .create_threadsafe_function(0, |ctx: ThreadSafeCallContext<String>| {
                Ok(vec![ctx.env.create_string(&ctx.value)?.into_unknown()])
            })?;
        callback.unref(&env)?;
        abort.unref(&env)?;
        let inner = Arc::new(NodeEmbeddingProvider {
            descriptor,
            timeout: Duration::from_millis(timeout_ms as u64),
            callback,
            abort,
            next_request_id: AtomicU64::new(1),
        });
        let instance_id = a3s_code_core::host_env::HostEnv::system().next_id();
        let mut registry = embedding_provider_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, provider| provider.strong_count() > 0);
        registry.insert(instance_id.clone(), Arc::downgrade(&inner));
        drop(registry);
        Ok(Self { instance_id, inner })
    }
}

impl Drop for CallbackEmbeddingProvider {
    fn drop(&mut self) {
        let mut registry = embedding_provider_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let own_provider = Arc::downgrade(&self.inner);
        if registry
            .get(&self.instance_id)
            .is_some_and(|registered| registered.ptr_eq(&own_provider))
        {
            registry.remove(&self.instance_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_provider_failures_preserve_retry_categories() {
        assert!(matches!(
            provider_failure(NodeEmbeddingFailure {
                kind: "rate_limited".to_owned(),
                retry_after_ms: Some(25),
            }),
            EmbeddingProviderError::RateLimited {
                retry_after: Some(delay)
            } if delay == Duration::from_millis(25)
        ));
    }
}
