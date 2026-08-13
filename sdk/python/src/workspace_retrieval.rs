//! Typed Python bridge for session-bound workspace retrieval.

use super::*;
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult, WorkspaceRetrievalStatus,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult,
};
use async_trait::async_trait;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod types;
use types::*;

const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 30_000;
const MAX_PROVIDER_TIMEOUT_MS: u64 = 300_000;
const MAX_SEARCH_LIMIT: usize = 25;

struct PythonEmbeddingProvider {
    descriptor: EmbeddingProviderDescriptor,
    callback: PyObject,
    event_loop: PyObject,
    timeout: Duration,
}

struct PythonEmbeddingCancelGuard {
    future: PyObject,
    armed: bool,
}

impl PythonEmbeddingCancelGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PythonEmbeddingCancelGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = Python::with_gil(|py| self.future.call_method0(py, "cancel"));
        }
    }
}

#[async_trait]
impl EmbeddingProvider for PythonEmbeddingProvider {
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
        let future = Python::with_gil(|py| -> PyResult<PyObject> {
            let input_list = PyList::empty(py);
            for input in request.inputs() {
                let value = PyDict::new(py);
                value.set_item("id", input.id())?;
                value.set_item("text", input.text())?;
                input_list.append(value)?;
            }
            let payload = PyDict::new(py);
            payload.set_item("inputs", input_list)?;
            payload.set_item("text_bytes", request.text_bytes())?;
            let awaitable = self.callback.call1(py, (payload,))?;
            py.import("asyncio")?
                .call_method1(
                    "run_coroutine_threadsafe",
                    (awaitable, self.event_loop.bind(py)),
                )
                .map(Bound::unbind)
        })
        .map_err(|_| EmbeddingProviderError::Other)?;

        let wait_future = Python::with_gil(|py| future.clone_ref(py));
        let descriptor = self.descriptor.clone();
        let mut waiter = tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                let value = wait_future
                    .call_method0(py, "result")
                    .map_err(|_| EmbeddingProviderError::Other)?;
                parse_embedding_response(py, value, &descriptor)
            })
        });
        let mut cancel_guard = PythonEmbeddingCancelGuard {
            future,
            armed: true,
        };

        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(EmbeddingProviderError::Cancelled),
            result = tokio::time::timeout(self.timeout, &mut waiter) => match result {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(EmbeddingProviderError::Other),
                Err(_) => Err(EmbeddingProviderError::Timeout),
            },
        };
        if result.is_ok() {
            cancel_guard.disarm();
        }
        result
    }
}

fn parse_embedding_response(
    py: Python<'_>,
    response: PyObject,
    descriptor: &EmbeddingProviderDescriptor,
) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
    let bound = response.bind(py);
    let dict = bound
        .downcast::<PyDict>()
        .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
    if let Some(kind) = dict
        .get_item("kind")
        .map_err(|_| EmbeddingProviderError::InvalidRequest)?
    {
        let kind = kind
            .extract::<String>()
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        let retry_after = dict
            .get_item("retry_after_ms")
            .ok()
            .flatten()
            .and_then(|value| value.extract::<u64>().ok())
            .map(Duration::from_millis);
        return Err(match kind.as_str() {
            "cancelled" => EmbeddingProviderError::Cancelled,
            "timeout" => EmbeddingProviderError::Timeout,
            "rate_limited" => EmbeddingProviderError::RateLimited { retry_after },
            "unavailable" => EmbeddingProviderError::Unavailable { retry_after },
            "authentication" => EmbeddingProviderError::Authentication,
            "invalid_request" => EmbeddingProviderError::InvalidRequest,
            _ => EmbeddingProviderError::Other,
        });
    }
    let vectors_value = dict
        .get_item("vectors")
        .map_err(|_| EmbeddingProviderError::InvalidRequest)?
        .ok_or(EmbeddingProviderError::InvalidRequest)?;
    let vectors = vectors_value
        .downcast::<PyList>()
        .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
    let mut parsed = Vec::with_capacity(vectors.len());
    for value in vectors {
        let value = value
            .downcast::<PyDict>()
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        let id = value
            .get_item("id")
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?
            .ok_or(EmbeddingProviderError::InvalidRequest)?
            .extract::<String>()
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        let values = value
            .get_item("values")
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?
            .ok_or(EmbeddingProviderError::InvalidRequest)?
            .extract::<Vec<f32>>()
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        parsed.push(EmbeddingVector::new(id, values));
    }
    Ok(EmbeddingBatchResponse::new(descriptor.clone(), parsed))
}

/// Host-injected async embedding callable and its immutable descriptor.
#[pyclass(name = "CallbackEmbeddingProvider")]
pub(super) struct PyCallbackEmbeddingProvider {
    provider: String,
    model: String,
    revision: Option<String>,
    dimension: usize,
    normalization: String,
    callback: PyObject,
    timeout_ms: u64,
}

impl Clone for PyCallbackEmbeddingProvider {
    fn clone(&self) -> Self {
        Python::with_gil(|py| Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            revision: self.revision.clone(),
            dimension: self.dimension,
            normalization: self.normalization.clone(),
            callback: self.callback.clone_ref(py),
            timeout_ms: self.timeout_ms,
        })
    }
}

#[pymethods]
impl PyCallbackEmbeddingProvider {
    #[new]
    #[pyo3(signature = (provider, model, dimension, embed, revision=None, normalization="none", timeout_ms=DEFAULT_PROVIDER_TIMEOUT_MS))]
    fn new(
        provider: String,
        model: String,
        dimension: usize,
        embed: PyObject,
        revision: Option<String>,
        normalization: &str,
        timeout_ms: u64,
    ) -> PyResult<Self> {
        if provider.trim().is_empty() || model.trim().is_empty() || dimension == 0 {
            return Err(PyValueError::new_err(
                "provider, model, and a positive dimension are required",
            ));
        }
        if !matches!(normalization, "none" | "unit") {
            return Err(PyValueError::new_err(
                "normalization must be 'none' or 'unit'",
            ));
        }
        if timeout_ms == 0 || timeout_ms > MAX_PROVIDER_TIMEOUT_MS {
            return Err(PyValueError::new_err(format!(
                "timeout_ms must be from 1 to {MAX_PROVIDER_TIMEOUT_MS}"
            )));
        }
        Python::with_gil(|py| {
            if !embed.bind(py).is_callable() {
                return Err(PyTypeError::new_err("embed must be callable"));
            }
            Ok(Self {
                provider,
                model,
                revision,
                dimension,
                normalization: normalization.to_owned(),
                callback: embed,
                timeout_ms,
            })
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "CallbackEmbeddingProvider(provider={:?}, model={:?}, dimension={})",
            self.provider, self.model, self.dimension
        )
    }
}

#[pyclass(name = "WorkspaceRetrievalOptions")]
#[derive(Clone)]
pub(super) struct PyWorkspaceRetrievalOptions {
    pub(super) provider: PyCallbackEmbeddingProvider,
    #[pyo3(get, set)]
    pub(super) max_records: usize,
    #[pyo3(get, set)]
    pub(super) max_bytes: usize,
    #[pyo3(get, set)]
    pub(super) shutdown_timeout_ms: u64,
}

#[pymethods]
impl PyWorkspaceRetrievalOptions {
    #[new]
    fn new(provider: PyRef<'_, PyCallbackEmbeddingProvider>) -> Self {
        Self {
            provider: provider.clone(),
            max_records: 100_000,
            max_bytes: 128 * 1024 * 1024,
            shutdown_timeout_ms: 5_000,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "WorkspaceRetrievalOptions(max_records={}, max_bytes={}, shutdown_timeout_ms={})",
            self.max_records, self.max_bytes, self.shutdown_timeout_ms
        )
    }
}

pub(super) fn retrieval_options_to_core(
    py: Python<'_>,
    options: &PyWorkspaceRetrievalOptions,
    event_loop: PyObject,
) -> PyResult<a3s_code_core::WorkspaceRetrievalOptions> {
    if options.max_records == 0 || options.max_bytes == 0 {
        return Err(PyValueError::new_err(
            "workspace retrieval memory limits must be positive",
        ));
    }
    if options.shutdown_timeout_ms == 0 || options.shutdown_timeout_ms > 30_000 {
        return Err(PyValueError::new_err(
            "shutdown_timeout_ms must be from 1 to 30000",
        ));
    }
    let normalization = match options.provider.normalization.as_str() {
        "unit" => EmbeddingNormalization::Unit,
        _ => EmbeddingNormalization::None,
    };
    let descriptor = EmbeddingProviderDescriptor {
        provider: options.provider.provider.clone(),
        model: options.provider.model.clone(),
        revision: options.provider.revision.clone(),
        dimension: options.provider.dimension,
        normalization,
    };
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(PythonEmbeddingProvider {
        descriptor,
        callback: options.provider.callback.clone_ref(py),
        event_loop,
        timeout: Duration::from_millis(options.provider.timeout_ms),
    });
    Ok(
        a3s_code_core::WorkspaceRetrievalOptions::new(provider).with_index_limits(
            a3s_code_core::WorkspaceSemanticIndexLimits {
                max_records: options.max_records,
                max_bytes: options.max_bytes,
                shutdown_timeout: Duration::from_millis(options.shutdown_timeout_ms),
            },
        ),
    )
}

enum AsyncWorkspaceRetrievalOperation {
    Semantic(WorkspaceSemanticSearchRequest),
    Hybrid(WorkspaceHybridSearchRequest),
}

#[pyclass]
struct AsyncWorkspaceRetrievalCall {
    session: Arc<RustAgentSession>,
    operation: Option<AsyncWorkspaceRetrievalOperation>,
}

#[pymethods]
impl AsyncWorkspaceRetrievalCall {
    fn __call__(&mut self, py: Python<'_>) -> PyResult<PyObject> {
        let operation = self
            .operation
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("workspace retrieval call already consumed"))?;
        let session = Arc::clone(&self.session);
        let value = py.allow_threads(move || match operation {
            AsyncWorkspaceRetrievalOperation::Semantic(request) => get_runtime()
                .block_on(session.semantic_search(request))
                .map(semantic_result_json),
            AsyncWorkspaceRetrievalOperation::Hybrid(request) => get_runtime()
                .block_on(session.hybrid_search(request))
                .map(hybrid_result_json),
        });
        let value = value.map_err(retrieval_error)?;
        let json = serde_json::to_string(&value)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }
}

#[pymethods]
impl PySession {
    /// Return a non-sensitive snapshot of the session-owned semantic index.
    fn workspace_retrieval_status(&self, py: Python<'_>) -> PyResult<PyObject> {
        let value = status_json(&self.inner.workspace_retrieval_status());
        let json = serde_json::to_string(&value)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }

    /// Return an asyncio Future for digest-verified semantic workspace search.
    fn semantic_search_async<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyDict>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncWorkspaceRetrievalCall {
                session: Arc::clone(&self.inner),
                operation: Some(AsyncWorkspaceRetrievalOperation::Semantic(
                    semantic_request(request)?,
                )),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Return an asyncio Future for Rust-owned exact/BM25/symbol/semantic fusion.
    fn hybrid_search_async<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyDict>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncWorkspaceRetrievalCall {
                session: Arc::clone(&self.inner),
                operation: Some(AsyncWorkspaceRetrievalOperation::Hybrid(hybrid_request(
                    request,
                )?)),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_rejects_unknown_normalization() {
        Python::with_gil(|py| {
            let callback = py
                .eval(c"lambda request: None", None, None)
                .unwrap()
                .unbind();
            assert!(PyCallbackEmbeddingProvider::new(
                "test".to_owned(),
                "fixture".to_owned(),
                4,
                callback,
                None,
                "mystery",
                30_000,
            )
            .is_err());
        });
    }
}
