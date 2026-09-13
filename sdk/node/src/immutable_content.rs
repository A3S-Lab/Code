//! Host-injected immutable content adapter for SessionOptions (SDK-IMM1).

use a3s_code_core::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1, ImmutableContentAdapterSession,
    ImmutableContentError, ImmutableContentReferenceV1, ImmutableContentResult,
    ImmutableContentWriteRequestV1, SdkImmutableContentWriteRequestV1,
};
use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction};
use napi::{Env, JsFunction};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::js_callback_bridge::wrap_sync_callback;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

type AdapterRegistry = HashMap<String, Weak<NodeImmutableContentAdapter>>;

fn adapter_registry() -> &'static Mutex<AdapterRegistry> {
    static REGISTRY: OnceLock<Mutex<AdapterRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_adapter(adapter: &Arc<NodeImmutableContentAdapter>) -> String {
    let instance_id = format!(
        "imm-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    if let Ok(mut registry) = adapter_registry().lock() {
        registry.insert(instance_id.clone(), Arc::downgrade(adapter));
    }
    instance_id
}

fn resolve_adapter(instance_id: &str) -> napi::Result<Arc<NodeImmutableContentAdapter>> {
    if instance_id.trim().is_empty() {
        return Err(napi::Error::from_reason(
            "immutableContentAdapter requires the original ImmutableContentAdapterOptions instance",
        ));
    }
    let weak = {
        let mut registry = adapter_registry()
            .lock()
            .map_err(|_| napi::Error::from_reason("immutable content adapter registry poisoned"))?;
        registry.retain(|_, weak| weak.strong_count() > 0);
        registry
            .get(instance_id)
            .cloned()
            .ok_or_else(|| {
                napi::Error::from_reason(
                    "immutableContentAdapter instance is no longer alive; keep the options object reachable until the session is created",
                )
            })?
    };
    weak.upgrade().ok_or_else(|| {
        napi::Error::from_reason(
            "immutableContentAdapter instance was dropped before session creation",
        )
    })
}

/// Structural SessionOptions shape. Pass an `ImmutableContentAdapterOptions` instance.
#[napi(object)]
#[derive(Clone, Default)]
pub struct ImmutableContentAdapterOptionsObject {
    pub instance_id: String,
}

struct NodeImmutableContentAdapter {
    name: String,
    binding: ImmutableContentAdapterBindingV1,
    timeout: Duration,
    callback: ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal>,
}

#[async_trait]
impl ImmutableContentAdapter for NodeImmutableContentAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        use napi::bindgen_prelude::Promise;

        let wire = SdkImmutableContentWriteRequestV1::from_request(request);
        let value = serde_json::to_value(&wire).map_err(|error| {
            ImmutableContentError::Provider(format!(
                "serialize immutable content write request: {error}"
            ))
        })?;
        let callback = self
            .callback
            .call_async::<Promise<serde_json::Value>>(value);
        let resolved = tokio::time::timeout(self.timeout, callback)
            .await
            .map_err(|_| ImmutableContentError::Provider("immutable content put timed out".into()))?
            .map_err(|error| {
                ImmutableContentError::Provider(format!(
                    "immutable content put callback failed: {error}"
                ))
            })?
            .await
            .map_err(|error| {
                ImmutableContentError::Provider(format!(
                    "immutable content put promise rejected: {error}"
                ))
            })?;
        serde_json::from_value::<ImmutableContentReferenceV1>(resolved).map_err(|error| {
            ImmutableContentError::Provider(format!(
                "immutable content put returned an invalid reference: {error}"
            ))
        })
    }
}

/// Host-owned create-only retention adapter for SessionOptions (SDK-IMM1).
#[napi]
pub struct ImmutableContentAdapterOptions {
    pub(crate) instance_id: String,
    _adapter: Arc<NodeImmutableContentAdapter>,
}

#[napi]
impl ImmutableContentAdapterOptions {
    #[napi(
        constructor,
        ts_args_type = "authorityDigest: string, maximumBytes: number, adapterName: string, put: (request: { binding: any; descriptor: any; contentBase64: string }) => any, timeoutMs?: number | null"
    )]
    pub fn new(
        env: Env,
        authority_digest: String,
        maximum_bytes: f64,
        adapter_name: String,
        put: JsFunction,
        timeout_ms: Option<f64>,
    ) -> napi::Result<Self> {
        if !maximum_bytes.is_finite() || maximum_bytes <= 0.0 {
            return Err(napi::Error::from_reason(
                "ImmutableContentAdapterOptions.maximumBytes must be a positive number",
            ));
        }
        let maximum_bytes = maximum_bytes as u64;
        let binding = ImmutableContentAdapterBindingV1::new(authority_digest, maximum_bytes)
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        let adapter_name = adapter_name.trim().to_string();
        if adapter_name.is_empty() {
            return Err(napi::Error::from_reason(
                "ImmutableContentAdapterOptions.adapterName must not be empty",
            ));
        }
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS as f64) as u64;
        if timeout_ms == 0 || timeout_ms > MAX_TIMEOUT_MS {
            return Err(napi::Error::from_reason(format!(
                "ImmutableContentAdapterOptions.timeoutMs must be from 1 to {MAX_TIMEOUT_MS}"
            )));
        }
        let safe = wrap_sync_callback(&env, put)?;
        let single_obj = |ctx: ThreadSafeCallContext<serde_json::Value>| {
            Ok(vec![ctx.env.to_js_value(&ctx.value)?])
        };
        let mut tsfn: ThreadsafeFunction<serde_json::Value, ErrorStrategy::Fatal> =
            safe.create_threadsafe_function(0, single_obj)?;
        tsfn.unref(&env)?;

        let adapter = Arc::new(NodeImmutableContentAdapter {
            name: adapter_name,
            binding,
            timeout: Duration::from_millis(timeout_ms),
            callback: tsfn,
        });
        let instance_id = register_adapter(&adapter);
        Ok(Self {
            instance_id,
            _adapter: adapter,
        })
    }

    #[napi(getter)]
    pub fn instance_id(&self) -> String {
        self.instance_id.clone()
    }
}

pub(crate) fn js_immutable_content_to_rust(
    options: &ImmutableContentAdapterOptionsObject,
) -> napi::Result<ImmutableContentAdapterSession> {
    let adapter = resolve_adapter(&options.instance_id)?;
    let binding = adapter.binding.clone();
    let port: Arc<dyn ImmutableContentAdapter> = adapter;
    ImmutableContentAdapterSession::new(binding, port)
        .map_err(|error| napi::Error::from_reason(error.to_string()))
}
