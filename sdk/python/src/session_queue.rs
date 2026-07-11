use super::*;

// ============================================================================
// SessionQueueConfig
// ============================================================================

/// Configuration for the optional advanced session lane queue.
///
/// Ordinary sessions do not initialize queue infrastructure. Use this only for
/// explicit external/hybrid dispatch, priority experiments, or operational integrations.
#[pyclass(name = "SessionQueueConfig")]
#[derive(Clone)]
pub(super) struct PySessionQueueConfig {
    pub(super) inner: RustSessionQueueConfig,
}

#[pymethods]
impl PySessionQueueConfig {
    #[new]
    fn new() -> Self {
        Self {
            inner: RustSessionQueueConfig::default(),
        }
    }

    /// Enable all lane features (DLQ, metrics, alerts) with sensible defaults.
    fn with_lane_features(&mut self) {
        self.inner = self.inner.clone().with_lane_features();
    }

    /// Set max concurrency for Query lane (default: 4).
    fn set_query_concurrency(&mut self, n: usize) {
        self.inner.query_max_concurrency = n;
    }

    /// Set max concurrency for Execute lane (default: 2).
    fn set_execute_concurrency(&mut self, n: usize) {
        self.inner.execute_max_concurrency = n;
    }

    /// Set max concurrency for Generate lane (default: 1).
    fn set_generate_concurrency(&mut self, n: usize) {
        self.inner.generate_max_concurrency = n;
    }

    /// Enable dead letter queue with optional max size.
    #[pyo3(signature = (max_size=None))]
    fn enable_dlq(&mut self, max_size: Option<usize>) {
        self.inner = self.inner.clone().with_dlq(max_size);
    }

    /// Enable metrics collection.
    fn enable_metrics(&mut self) {
        self.inner = self.inner.clone().with_metrics();
    }

    /// Enable queue alerts.
    fn enable_alerts(&mut self) {
        self.inner = self.inner.clone().with_alerts();
    }

    /// Set default timeout for commands (ms).
    fn set_timeout(&mut self, timeout_ms: u64) {
        self.inner = self.inner.clone().with_timeout(timeout_ms);
    }

    /// Configure how a specific lane handles tasks.
    ///
    /// Args:
    ///     lane (Literal["control", "query", "execute", "generate"]): Which lane to configure.
    ///     mode (Literal["internal", "external", "hybrid"]): Execution mode for the lane's tools.
    ///     timeout_ms: Timeout for external tasks in milliseconds (default 60000).
    #[pyo3(signature = (lane, mode, timeout_ms=60_000))]
    fn set_lane_handler(&mut self, lane: &str, mode: &str, timeout_ms: u64) -> PyResult<()> {
        let rust_lane = parse_lane(lane)?;
        let rust_mode = parse_handler_mode(mode)?;
        let config = RustLaneHandlerConfig {
            mode: rust_mode,
            timeout_ms,
        };
        self.inner.lane_handlers.insert(rust_lane, config);
        Ok(())
    }

    /// Set max concurrency for Query lane (default: 4).
    #[getter]
    fn get_query_max_concurrency(&self) -> usize {
        self.inner.query_max_concurrency
    }

    #[setter]
    fn set_query_max_concurrency(&mut self, value: usize) {
        self.inner.query_max_concurrency = value;
    }

    fn __repr__(&self) -> String {
        format!(
            "SessionQueueConfig(query={}, execute={}, generate={}, dlq={}, metrics={})",
            self.inner.query_max_concurrency,
            self.inner.execute_max_concurrency,
            self.inner.generate_max_concurrency,
            self.inner.enable_dlq,
            self.inner.enable_metrics,
        )
    }
}

// ============================================================================
// Queue Helpers
// ============================================================================

pub(super) fn parse_lane(lane: &str) -> PyResult<RustSessionLane> {
    match lane {
        "control" => Ok(RustSessionLane::Control),
        "query" => Ok(RustSessionLane::Query),
        "execute" => Ok(RustSessionLane::Execute),
        "generate" => Ok(RustSessionLane::Generate),
        _ => Err(PyValueError::new_err(format!(
            "Invalid lane '{}'. Must be: control, query, execute, or generate",
            lane
        ))),
    }
}

pub(super) fn parse_handler_mode(mode: &str) -> PyResult<RustTaskHandlerMode> {
    match mode {
        "internal" => Ok(RustTaskHandlerMode::Internal),
        "external" => Ok(RustTaskHandlerMode::External),
        "hybrid" => Ok(RustTaskHandlerMode::Hybrid),
        _ => Err(PyValueError::new_err(format!(
            "Invalid handler mode '{}'. Must be: internal, external, or hybrid",
            mode
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_parsers_accept_catalog_values_and_reject_unknown_values() {
        assert_eq!(parse_lane("control").unwrap(), RustSessionLane::Control);
        assert_eq!(parse_lane("query").unwrap(), RustSessionLane::Query);
        assert_eq!(parse_lane("execute").unwrap(), RustSessionLane::Execute);
        assert_eq!(parse_lane("generate").unwrap(), RustSessionLane::Generate);
        assert!(parse_lane("background").is_err());

        assert_eq!(
            parse_handler_mode("internal").unwrap(),
            RustTaskHandlerMode::Internal
        );
        assert_eq!(
            parse_handler_mode("external").unwrap(),
            RustTaskHandlerMode::External
        );
        assert_eq!(
            parse_handler_mode("hybrid").unwrap(),
            RustTaskHandlerMode::Hybrid
        );
        assert!(parse_handler_mode("automatic").is_err());
    }
}
