//! Node Session memory methods.

use super::session::Session;
use super::*;

#[napi]
impl Session {
    // ========================================================================
    // Memory API
    // ========================================================================

    /// Check if memory is available for this session.
    #[napi(getter)]
    pub fn has_memory(&self) -> bool {
        self.inner.memory().is_some()
    }

    /// Remember a successful task execution.
    ///
    /// @param task - Description of the task
    /// @param tools - List of tool names used
    /// @param result - Summary of the result
    #[napi]
    pub async fn remember_success(
        &self,
        task: String,
        tools: Vec<String>,
        result: String,
    ) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        get_runtime()
            .spawn(async move { memory.remember_success(&task, &tools, &result).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Remember failed: {e}")))
    }

    /// Remember a failed task execution.
    ///
    /// @param task - Description of the task
    /// @param error - Error message
    /// @param tools - List of tool names attempted
    #[napi]
    pub async fn remember_failure(
        &self,
        task: String,
        error: String,
        tools: Vec<String>,
    ) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        get_runtime()
            .spawn(async move { memory.remember_failure(&task, &error, &tools).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Remember failed: {e}")))
    }

    /// Recall memories similar to a query.
    ///
    /// @param query - Search query
    /// @param limit - Maximum number of results (default: 5)
    /// @returns Array of memory items
    #[napi]
    pub async fn recall_similar(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let limit = limit.unwrap_or(5) as usize;
        let items = get_runtime()
            .spawn(async move { memory.recall_similar(&query, limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Recall memories by tags.
    ///
    /// @param tags - Tags to search for
    /// @param limit - Maximum number of results (default: 10)
    /// @returns Array of memory items
    #[napi]
    pub async fn recall_by_tags(
        &self,
        tags: Vec<String>,
        limit: Option<u32>,
    ) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let limit = limit.unwrap_or(10) as usize;
        let items = get_runtime()
            .spawn(async move { memory.recall_by_tags(&tags, limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get recent memory items.
    ///
    /// @param limit - Maximum number of results (default: 10)
    /// @returns Array of memory items
    #[napi]
    pub async fn memory_recent(&self, limit: Option<u32>) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let limit = limit.unwrap_or(10) as usize;
        let items = get_runtime()
            .spawn(async move { memory.get_recent(limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get memory statistics.
    ///
    /// @returns Object with longTermCount, shortTermCount, workingCount
    #[napi]
    pub async fn memory_stats(&self) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let stats = get_runtime()
            .spawn(async move { memory.stats().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Stats failed: {e}")))?;
        serde_json::to_value(&stats)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get current working memory items.
    ///
    /// Working memory holds the active context items for the current task.
    ///
    /// @returns Array of memory items currently in working memory
    #[napi]
    pub async fn get_working(&self) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = get_runtime()
            .spawn(async move { memory.get_working().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Clear working memory.
    ///
    /// Removes all items from working memory without affecting short-term or long-term memory.
    #[napi]
    pub async fn clear_working(&self) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        get_runtime()
            .spawn(async move { memory.clear_working().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    /// Get current short-term memory items.
    ///
    /// Short-term memory contains items stored during this session.
    ///
    /// @returns Array of memory items in short-term memory
    #[napi]
    pub async fn get_short_term(&self) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        let items = get_runtime()
            .spawn(async move { memory.get_short_term().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Clear short-term memory for this session.
    ///
    /// Removes all session-scoped memory items without affecting long-term or working memory.
    #[napi]
    pub async fn clear_short_term(&self) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason(MEMORY_UNAVAILABLE_MESSAGE))?
            .clone();
        get_runtime()
            .spawn(async move { memory.clear_short_term().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }
}
