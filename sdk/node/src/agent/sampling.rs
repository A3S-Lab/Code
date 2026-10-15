//! Promise-based, exception-contained MCP sampling host bridge.

use a3s_code_core::mcp::{
    client::BoundedSamplingHandler, transport::SamplingHandler, SamplingHandlerFactory,
};
use napi::{
    bindgen_prelude::Promise,
    threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction},
    Env, JsFunction,
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

// Always resolve a string, including when the host throws, rejects or returns
// a non-JSON value. No fallible host-value conversion crosses the TSFN return
// boundary, where napi-rs would otherwise abort the Node process.
const WRAPPER: &str = r#"(callback => async (server, input) => {
  try {
    const value = await callback(server, JSON.parse(input));
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      throw new TypeError('sampling result must be an object');
    }
    return JSON.stringify({ok: true, value});
  } catch (_) {
    return '{"ok":false}';
  }
})"#;

struct NodeSampling {
    server: String,
    callback: Arc<ThreadsafeFunction<(String, String), ErrorStrategy::Fatal>>,
    timeout: Duration,
}

#[async_trait::async_trait]
impl SamplingHandler for NodeSampling {
    async fn create_message(&self, params: Value) -> anyhow::Result<Value> {
        let input = serde_json::to_string(&params)?;
        let response = tokio::time::timeout(self.timeout, async {
            self.callback
                .call_async::<Promise<String>>((self.server.clone(), input))
                .await?
                .await
        })
        .await
        .map_err(|_| anyhow::anyhow!("sampling host callback timed out"))?
        .map_err(|_| anyhow::anyhow!("sampling host callback failed"))?;
        let envelope: Value = serde_json::from_str(&response)?;
        anyhow::ensure!(
            envelope.get("ok") == Some(&Value::Bool(true)),
            "sampling refused by host callback"
        );
        let value = envelope
            .get("value")
            .filter(|value| value.is_object())
            .ok_or_else(|| anyhow::anyhow!("sampling host returned an invalid result"))?;
        Ok(value.clone())
    }
}

pub(super) fn factory(
    env: &Env,
    handler: JsFunction,
    timeout_ms: Option<u32>,
) -> napi::Result<SamplingHandlerFactory> {
    let timeout_ms = timeout_ms.unwrap_or(30_000);
    if timeout_ms == 0 || timeout_ms > 300_000 {
        return Err(napi::Error::from_reason(
            "sampling timeoutMs must be from 1 to 300000",
        ));
    }
    let wrapper: JsFunction = env.run_script(WRAPPER)?;
    let safe = JsFunction::try_from(wrapper.call(None, &[handler])?)?;
    let mut callback: ThreadsafeFunction<(String, String), ErrorStrategy::Fatal> = safe
        .create_threadsafe_function(1, |ctx: ThreadSafeCallContext<(String, String)>| {
            Ok(vec![
                ctx.env.create_string(&ctx.value.0)?,
                ctx.env.create_string(&ctx.value.1)?,
            ])
        })?;
    callback.unref(env)?;
    let callback = Arc::new(callback);
    Ok(Arc::new(move |server| {
        Arc::new(BoundedSamplingHandler::new(Arc::new(NodeSampling {
            server: server.to_owned(),
            callback: callback.clone(),
            timeout: Duration::from_millis(u64::from(timeout_ms)),
        })))
    }))
}
