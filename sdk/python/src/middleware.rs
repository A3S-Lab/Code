//! Middleware bindings for Python
//!
//! PyO3 bindings for the Rust Core middleware system.

use a3s_code_core::middleware::{
    LoggingMiddleware as RustLoggingMiddleware, Middleware as RustMiddleware,
    MiddlewareContext as RustMiddlewareContext, MiddlewarePipeline as RustMiddlewarePipeline,
    MiddlewareResult as RustMiddlewareResult,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// MiddlewareContext
// ============================================================================

/// Middleware execution context
#[pyclass(name = "MiddlewareContext")]
#[derive(Clone)]
pub struct PyMiddlewareContext {
    inner: RustMiddlewareContext,
}

#[pymethods]
impl PyMiddlewareContext {
    #[new]
    fn new(session_id: String, workspace: String) -> Self {
        Self {
            inner: RustMiddlewareContext::new(session_id, PathBuf::from(workspace)),
        }
    }

    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id.clone()
    }

    #[getter]
    fn workspace(&self) -> String {
        self.inner.workspace.to_string_lossy().to_string()
    }

    #[getter]
    fn prompt(&self) -> Option<String> {
        self.inner.prompt.clone()
    }

    #[setter]
    fn set_prompt(&mut self, prompt: Option<String>) {
        self.inner.prompt = prompt;
    }

    fn get_metadata(&self, key: &str) -> Option<String> {
        self.inner
            .get_metadata(key)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    fn set_metadata(&mut self, key: String, value: String) {
        self.inner
            .set_metadata(key, serde_json::Value::String(value));
    }

    fn __repr__(&self) -> String {
        format!(
            "MiddlewareContext(session_id='{}', workspace='{}')",
            self.inner.session_id,
            self.inner.workspace.display()
        )
    }
}

// ============================================================================
// MiddlewareResult
// ============================================================================

/// Middleware execution result
#[pyclass(name = "MiddlewareResult")]
pub struct PyMiddlewareResult;

#[pymethods]
impl PyMiddlewareResult {
    #[classattr]
    const CONTINUE: &'static str = "continue";

    #[classattr]
    const ABORT: &'static str = "abort";
}

// ============================================================================
// Middleware Trait (Python Callback)
// ============================================================================

/// Python middleware wrapper
pub struct PyMiddleware {
    callback: PyObject,
}

impl PyMiddleware {
    pub fn new(callback: PyObject) -> Self {
        Self { callback }
    }
}

#[async_trait::async_trait]
impl RustMiddleware for PyMiddleware {
    async fn handle(
        &self,
        ctx: &mut RustMiddlewareContext,
    ) -> anyhow::Result<RustMiddlewareResult> {
        Python::with_gil(|py| {
            // Convert Rust context to Python
            let py_ctx = PyMiddlewareContext {
                inner: ctx.clone(),
            };

            // Call Python callback
            let result = self.callback.call1(py, (py_ctx,))?;

            // Parse result
            if let Ok(dict) = result.downcast_bound::<PyDict>(py) {
                if let Some(result_type) = dict.get_item("type")? {
                    let type_str: String = result_type.extract()?;

                    match type_str.as_str() {
                        "continue" => Ok(RustMiddlewareResult::Continue),
                        "abort" => {
                            let reason = dict
                                .get_item("reason")?
                                .and_then(|r| r.extract::<String>().ok())
                                .unwrap_or_else(|| "Aborted by Python middleware".to_string());
                            Ok(RustMiddlewareResult::Abort(reason))
                        }
                        _ => Ok(RustMiddlewareResult::Continue),
                    }
                } else {
                    Ok(RustMiddlewareResult::Continue)
                }
            } else {
                Ok(RustMiddlewareResult::Continue)
            }
        })
    }

    fn name(&self) -> &str {
        "PyMiddleware"
    }
}

// ============================================================================
// MiddlewarePipeline
// ============================================================================

/// Middleware pipeline
#[pyclass(name = "MiddlewarePipeline")]
pub struct PyMiddlewarePipeline {
    inner: Arc<tokio::sync::Mutex<RustMiddlewarePipeline>>,
}

#[pymethods]
impl PyMiddlewarePipeline {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(RustMiddlewarePipeline::new())),
        }
    }

    fn use_middleware(&self, callback: PyObject) -> PyResult<()> {
        let middleware = Arc::new(PyMiddleware::new(callback));

        let inner = self.inner.clone();
        crate::get_runtime().block_on(async move {
            let mut pipeline = inner.lock().await;
            pipeline.use_middleware(middleware);
        });

        Ok(())
    }

    fn execute(&self, ctx: PyMiddlewareContext) -> PyResult<PyMiddlewareContext> {
        let inner = self.inner.clone();
        let mut rust_ctx = ctx.inner;

        let result = crate::get_runtime().block_on(async move {
            let pipeline = inner.lock().await;
            pipeline.execute(&mut rust_ctx).await?;
            Ok::<_, anyhow::Error>(rust_ctx)
        });

        match result {
            Ok(updated_ctx) => Ok(PyMiddlewareContext {
                inner: updated_ctx,
            }),
            Err(e) => Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                "Middleware execution failed: {}",
                e
            ))),
        }
    }

    fn __len__(&self) -> usize {
        let inner = self.inner.clone();
        crate::get_runtime().block_on(async move {
            let pipeline = inner.lock().await;
            pipeline.len()
        })
    }

    fn __repr__(&self) -> String {
        format!("MiddlewarePipeline(middleware_count={})", self.__len__())
    }
}

// ============================================================================
// Built-in Middleware
// ============================================================================

/// Logging middleware
#[pyclass(name = "LoggingMiddleware")]
pub struct PyLoggingMiddleware {
    inner: Arc<RustLoggingMiddleware>,
}

#[pymethods]
impl PyLoggingMiddleware {
    #[new]
    fn new(level: String) -> Self {
        Self {
            inner: Arc::new(RustLoggingMiddleware::new(level)),
        }
    }
}

// ============================================================================
// Module Registration
// ============================================================================

pub fn register_middleware_classes(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMiddlewareContext>()?;
    m.add_class::<PyMiddlewareResult>()?;
    m.add_class::<PyMiddlewarePipeline>()?;
    m.add_class::<PyLoggingMiddleware>()?;
    Ok(())
}
