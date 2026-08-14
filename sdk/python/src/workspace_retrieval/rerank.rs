use super::*;

/// Explicit bounded deterministic second-stage ranking for hybrid search.
///
/// Leave `WorkspaceRetrievalOptions.reranker` unset to preserve RRF-only.
#[pyclass(name = "DeterministicWorkspaceReranker")]
#[derive(Clone)]
pub(crate) struct PyDeterministicWorkspaceReranker {
    #[pyo3(get, set)]
    pub(super) max_candidates: usize,
    #[pyo3(get, set)]
    pub(super) max_feature_bytes_per_candidate: usize,
    #[pyo3(get, set)]
    pub(super) max_fingerprints_per_candidate: usize,
    #[pyo3(get, set)]
    pub(super) max_scratch_bytes: usize,
}

#[pymethods]
impl PyDeterministicWorkspaceReranker {
    #[new]
    fn new() -> Self {
        let defaults = a3s_code_core::WorkspaceRerankOptions::deterministic();
        Self {
            max_candidates: defaults.max_candidates,
            max_feature_bytes_per_candidate: defaults.max_feature_bytes_per_candidate,
            max_fingerprints_per_candidate: defaults.max_fingerprints_per_candidate,
            max_scratch_bytes: defaults.max_scratch_bytes,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "DeterministicWorkspaceReranker(max_candidates={}, max_feature_bytes_per_candidate={}, max_fingerprints_per_candidate={}, max_scratch_bytes={})",
            self.max_candidates,
            self.max_feature_bytes_per_candidate,
            self.max_fingerprints_per_candidate,
            self.max_scratch_bytes,
        )
    }
}

pub(super) fn deterministic_reranker_to_core(
    options: &PyDeterministicWorkspaceReranker,
) -> PyResult<a3s_code_core::WorkspaceRerankOptions> {
    a3s_code_core::WorkspaceRerankOptions::deterministic()
        .with_max_candidates(options.max_candidates)
        .with_max_feature_bytes_per_candidate(options.max_feature_bytes_per_candidate)
        .with_max_fingerprints_per_candidate(options.max_fingerprints_per_candidate)
        .with_max_scratch_bytes(options.max_scratch_bytes)
        .validate()
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_object_matches_core_deterministic_options() {
        assert_eq!(
            deterministic_reranker_to_core(&PyDeterministicWorkspaceReranker::new())
                .expect("default reranker"),
            a3s_code_core::WorkspaceRerankOptions::deterministic()
        );
    }

    #[test]
    fn invalid_candidate_bound_is_rejected() {
        let mut reranker = PyDeterministicWorkspaceReranker::new();
        reranker.max_candidates = 0;
        assert!(deterministic_reranker_to_core(&reranker).is_err());
    }
}
