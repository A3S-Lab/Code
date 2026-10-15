//! Asyncio-owned MCP sampling callbacks with bounded waits and cancellation.

use super::super::*;
use a3s_code_core::mcp::{
    client::BoundedSamplingHandler, transport::SamplingHandler, SamplingHandlerFactory,
};
use serde_json::Value;
use std::time::Duration;

struct PythonSampling {
    server: String,
    callback: PyObject,
    event_loop: PyObject,
    timeout: Duration,
}

struct CancelFuture(PyObject);
impl Drop for CancelFuture {
    fn drop(&mut self) {
        let _ = Python::with_gil(|py| self.0.call_method0(py, "cancel"));
    }
}

#[async_trait::async_trait]
impl SamplingHandler for PythonSampling {
    async fn create_message(&self, params: Value) -> anyhow::Result<Value> {
        let input = serde_json::to_string(&params)?;
        let future = Python::with_gil(|py| -> PyResult<PyObject> {
            let invoke = py.import("a3s_code.mcp_sampling")?.getattr("_invoke")?;
            let awaitable = invoke.call1((self.callback.bind(py), &self.server, input))?;
            Ok(py
                .import("asyncio")?
                .call_method1(
                    "run_coroutine_threadsafe",
                    (awaitable, self.event_loop.bind(py)),
                )?
                .unbind())
        })
        .map_err(|_| anyhow::anyhow!("sampling host callback unavailable"))?;
        let wait_future = Python::with_gil(|py| future.clone_ref(py));
        let _cancel = CancelFuture(future);
        let timeout = self.timeout;
        let response = tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || {
                Python::with_gil(|py| {
                    wait_future
                        .call_method1(py, "result", (timeout.as_secs_f64(),))?
                        .extract::<String>(py)
                })
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("sampling host callback timed out"))?
        .map_err(|_| anyhow::anyhow!("sampling host callback worker failed"))?
        .map_err(|_| anyhow::anyhow!("sampling refused by host callback"))?;
        let value: Value = serde_json::from_str(&response)?;
        anyhow::ensure!(
            value.is_object(),
            "sampling host returned an invalid result"
        );
        Ok(value)
    }
}

pub(super) fn factory(
    py: Python<'_>,
    callback: PyObject,
    timeout_ms: u64,
) -> PyResult<SamplingHandlerFactory> {
    if timeout_ms == 0 || timeout_ms > 300_000 {
        return Err(PyValueError::new_err(
            "sampling timeout_ms must be from 1 to 300000",
        ));
    }
    if !callback.bind(py).is_callable() {
        return Err(PyTypeError::new_err("sampling handler must be callable"));
    }
    let event_loop = py
        .import("asyncio")?
        .call_method0("get_running_loop")?
        .unbind();
    Ok(Arc::new(move |server| {
        Python::with_gil(|py| {
            Arc::new(BoundedSamplingHandler::new(Arc::new(PythonSampling {
                server: server.to_owned(),
                callback: callback.clone_ref(py),
                event_loop: event_loop.clone_ref(py),
                timeout: Duration::from_millis(timeout_ms),
            }))) as Arc<dyn SamplingHandler>
        })
    }))
}
