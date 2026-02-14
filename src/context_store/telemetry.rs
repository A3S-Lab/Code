//! Telemetry for context store operations

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Metrics {
    pub nodes_ingested: AtomicU64,
    pub queries_executed: AtomicU64,
    pub embeddings_generated: AtomicU64,
    pub digests_generated: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self { Self::default() }

    pub fn record_ingest(&self) { self.nodes_ingested.fetch_add(1, Ordering::Relaxed); }
    pub fn record_query(&self) { self.queries_executed.fetch_add(1, Ordering::Relaxed); }
    pub fn record_embedding(&self) { self.embeddings_generated.fetch_add(1, Ordering::Relaxed); }
    pub fn record_digest(&self) { self.digests_generated.fetch_add(1, Ordering::Relaxed); }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            nodes_ingested: self.nodes_ingested.load(Ordering::Relaxed),
            queries_executed: self.queries_executed.load(Ordering::Relaxed),
            embeddings_generated: self.embeddings_generated.load(Ordering::Relaxed),
            digests_generated: self.digests_generated.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub nodes_ingested: u64,
    pub queries_executed: u64,
    pub embeddings_generated: u64,
    pub digests_generated: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_default() {
        let m = Metrics::new();
        let s = m.snapshot();
        assert_eq!(s.nodes_ingested, 0);
        assert_eq!(s.queries_executed, 0);
    }

    #[test]
    fn test_metrics_record() {
        let m = Metrics::new();
        m.record_ingest();
        m.record_ingest();
        m.record_query();
        let s = m.snapshot();
        assert_eq!(s.nodes_ingested, 2);
        assert_eq!(s.queries_executed, 1);
    }
}
