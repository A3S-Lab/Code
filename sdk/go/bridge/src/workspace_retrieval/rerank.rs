use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BridgeDeterministicWorkspaceReranker {
    max_candidates: Option<usize>,
    max_feature_bytes_per_candidate: Option<usize>,
    max_fingerprints_per_candidate: Option<usize>,
    max_scratch_bytes: Option<usize>,
}

impl BridgeDeterministicWorkspaceReranker {
    pub(super) fn into_core(self) -> Result<a3s_code_core::WorkspaceRerankOptions, BridgeFailure> {
        let defaults = a3s_code_core::WorkspaceRerankOptions::deterministic();
        a3s_code_core::WorkspaceRerankOptions::deterministic()
            .with_max_candidates(self.max_candidates.unwrap_or(defaults.max_candidates))
            .with_max_feature_bytes_per_candidate(
                self.max_feature_bytes_per_candidate
                    .unwrap_or(defaults.max_feature_bytes_per_candidate),
            )
            .with_max_fingerprints_per_candidate(
                self.max_fingerprints_per_candidate
                    .unwrap_or(defaults.max_fingerprints_per_candidate),
            )
            .with_max_scratch_bytes(self.max_scratch_bytes.unwrap_or(defaults.max_scratch_bytes))
            .validate()
            .map_err(|error| invalid_retrieval(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_fields_use_core_deterministic_defaults() {
        let options = BridgeDeterministicWorkspaceReranker::default()
            .into_core()
            .expect("default deterministic reranker");
        assert_eq!(
            options,
            a3s_code_core::WorkspaceRerankOptions::deterministic()
        );
    }

    #[test]
    fn invalid_bounds_fail_during_bridge_conversion() {
        let error = BridgeDeterministicWorkspaceReranker {
            max_candidates: Some(0),
            ..BridgeDeterministicWorkspaceReranker::default()
        }
        .into_core()
        .expect_err("zero candidates must fail");
        assert_eq!(error.code, "INVALID_REQUEST");
    }

    #[test]
    fn primitive_selector_fields_are_rejected() {
        for field in ["mode", "algorithm"] {
            let mut object = serde_json::Map::new();
            object.insert(
                field.to_owned(),
                serde_json::Value::String("deterministic".to_owned()),
            );
            assert!(
                serde_json::from_value::<BridgeDeterministicWorkspaceReranker>(
                    serde_json::Value::Object(object)
                )
                .is_err(),
                "unknown primitive selector {field} must fail closed"
            );
        }
    }
}
