use super::*;

type NodeRerankerRegistry =
    std::collections::HashMap<String, Weak<NodeDeterministicRerankerConfiguration>>;

pub(super) struct NodeDeterministicRerankerConfiguration {
    options: a3s_code_core::WorkspaceRerankOptions,
}

fn reranker_registry() -> &'static Mutex<NodeRerankerRegistry> {
    static REGISTRY: OnceLock<Mutex<NodeRerankerRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Explicit bounded deterministic second-stage ranking for hybrid search.
///
/// Omit this object from `WorkspaceRetrievalOptions` to preserve RRF-only.
#[napi]
pub struct DeterministicWorkspaceReranker {
    max_candidates: f64,
    max_feature_bytes_per_candidate: f64,
    max_fingerprints_per_candidate: f64,
    max_scratch_bytes: f64,
}

#[napi]
impl DeterministicWorkspaceReranker {
    #[napi(constructor)]
    pub fn new() -> Self {
        let defaults = a3s_code_core::WorkspaceRerankOptions::deterministic();
        Self {
            max_candidates: defaults.max_candidates as f64,
            max_feature_bytes_per_candidate: defaults.max_feature_bytes_per_candidate as f64,
            max_fingerprints_per_candidate: defaults.max_fingerprints_per_candidate as f64,
            max_scratch_bytes: defaults.max_scratch_bytes as f64,
        }
    }

    #[napi(getter)]
    pub fn max_candidates(&self) -> f64 {
        self.max_candidates
    }

    #[napi(setter)]
    pub fn set_max_candidates(&mut self, value: f64) {
        self.max_candidates = value;
    }

    #[napi(getter)]
    pub fn max_feature_bytes_per_candidate(&self) -> f64 {
        self.max_feature_bytes_per_candidate
    }

    #[napi(setter)]
    pub fn set_max_feature_bytes_per_candidate(&mut self, value: f64) {
        self.max_feature_bytes_per_candidate = value;
    }

    #[napi(getter)]
    pub fn max_fingerprints_per_candidate(&self) -> f64 {
        self.max_fingerprints_per_candidate
    }

    #[napi(setter)]
    pub fn set_max_fingerprints_per_candidate(&mut self, value: f64) {
        self.max_fingerprints_per_candidate = value;
    }

    #[napi(getter)]
    pub fn max_scratch_bytes(&self) -> f64 {
        self.max_scratch_bytes
    }

    #[napi(setter)]
    pub fn set_max_scratch_bytes(&mut self, value: f64) {
        self.max_scratch_bytes = value;
    }
}

pub(super) fn bind_deterministic_reranker(
    options: &DeterministicWorkspaceReranker,
) -> napi::Result<(String, Arc<NodeDeterministicRerankerConfiguration>)> {
    let options = deterministic_reranker_to_core(options)?;
    let configuration = Arc::new(NodeDeterministicRerankerConfiguration { options });
    let instance_id = a3s_code_core::host_env::HostEnv::system().next_id();
    let mut registry = reranker_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    registry.retain(|_, configuration| configuration.strong_count() > 0);
    registry.insert(instance_id.clone(), Arc::downgrade(&configuration));
    drop(registry);
    Ok((instance_id, configuration))
}

pub(super) fn resolve_deterministic_reranker(
    instance_id: &str,
) -> napi::Result<Option<a3s_code_core::WorkspaceRerankOptions>> {
    if instance_id.is_empty() {
        return Ok(None);
    }
    let weak = {
        let mut registry = reranker_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, configuration| configuration.strong_count() > 0);
        registry.get(instance_id).cloned()
    };
    weak.and_then(|configuration| configuration.upgrade())
        .map(|configuration| configuration.options)
        .map(Some)
        .ok_or_else(|| {
            napi::Error::from_reason(
                "WorkspaceRetrievalOptions reranker identity is invalid or expired; pass the original instance",
            )
        })
}

pub(super) fn unregister_deterministic_reranker(
    instance_id: &str,
    configuration: &Arc<NodeDeterministicRerankerConfiguration>,
) {
    if instance_id.is_empty() {
        return;
    }
    let own_configuration = Arc::downgrade(configuration);
    let mut registry = reranker_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if registry
        .get(instance_id)
        .is_some_and(|registered| registered.ptr_eq(&own_configuration))
    {
        registry.remove(instance_id);
    }
}

fn deterministic_reranker_to_core(
    options: &DeterministicWorkspaceReranker,
) -> napi::Result<a3s_code_core::WorkspaceRerankOptions> {
    let defaults = a3s_code_core::WorkspaceRerankOptions::deterministic();
    a3s_code_core::WorkspaceRerankOptions::deterministic()
        .with_max_candidates(js_optional_usize(
            Some(options.max_candidates),
            "workspaceRetrieval.reranker.maxCandidates",
            defaults.max_candidates,
        )?)
        .with_max_feature_bytes_per_candidate(js_optional_usize(
            Some(options.max_feature_bytes_per_candidate),
            "workspaceRetrieval.reranker.maxFeatureBytesPerCandidate",
            defaults.max_feature_bytes_per_candidate,
        )?)
        .with_max_fingerprints_per_candidate(js_optional_usize(
            Some(options.max_fingerprints_per_candidate),
            "workspaceRetrieval.reranker.maxFingerprintsPerCandidate",
            defaults.max_fingerprints_per_candidate,
        )?)
        .with_max_scratch_bytes(js_optional_usize(
            Some(options.max_scratch_bytes),
            "workspaceRetrieval.reranker.maxScratchBytes",
            defaults.max_scratch_bytes,
        )?)
        .validate()
        .map_err(|error| napi::Error::from_reason(error.to_string()))
}

impl Default for DeterministicWorkspaceReranker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_object_matches_core_deterministic_options() {
        assert_eq!(
            deterministic_reranker_to_core(&DeterministicWorkspaceReranker::new())
                .expect("default reranker"),
            a3s_code_core::WorkspaceRerankOptions::deterministic()
        );
    }

    #[test]
    fn invalid_candidate_bound_is_rejected() {
        let mut reranker = DeterministicWorkspaceReranker::new();
        reranker.max_candidates = 0.0;
        let error = deterministic_reranker_to_core(&reranker)
            .expect_err("zero candidates must be rejected");
        assert!(error.to_string().contains("rerank.max_candidates"));
    }
}
