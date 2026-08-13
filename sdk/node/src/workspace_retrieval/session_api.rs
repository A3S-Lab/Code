use super::*;

fn retrieval_error(error: a3s_code_core::WorkspaceRetrievalError) -> napi::Error {
    napi::Error::from_reason(format!(
        "[A3S_CODE_ERROR:WORKSPACE_RETRIEVAL_ERROR] {error}"
    ))
}

#[napi]
impl Session {
    /// Return a non-sensitive snapshot of the session-owned semantic index.
    #[napi]
    pub fn workspace_retrieval_status(&self) -> WorkspaceRetrievalStatusObject {
        self.inner.workspace_retrieval_status().into()
    }

    /// Search the current, digest-verified workspace using semantic similarity.
    #[napi]
    pub async fn semantic_search(
        &self,
        request: WorkspaceSearchRequest,
    ) -> napi::Result<WorkspaceSemanticSearchResultObject> {
        self.inner
            .semantic_search(semantic_request(request)?)
            .await
            .map(Into::into)
            .map_err(retrieval_error)
    }

    /// Fuse exact, BM25, symbol, and optional semantic evidence in Rust Core.
    #[napi]
    pub async fn hybrid_search(
        &self,
        request: WorkspaceSearchRequest,
    ) -> napi::Result<WorkspaceHybridSearchResultObject> {
        self.inner
            .hybrid_search(hybrid_request(request)?)
            .await
            .map(Into::into)
            .map_err(retrieval_error)
    }
}
