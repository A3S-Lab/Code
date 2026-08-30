use serde::Serialize;
use std::time::Duration;

/// Maximum number of per-run semantic refresh observations retained per owner.
pub const SEMANTIC_REFRESH_RECENT_RUN_LIMIT: usize = 64;

/// Terminal result of one scheduled semantic refresh attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRefreshRunOutcome {
    Published,
    Unchanged,
    Failed,
}

/// Non-sensitive work evidence for one settled scheduled semantic refresh
/// attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRefreshRunMetrics {
    sequence: u64,
    outcome: SemanticRefreshRunOutcome,
    elapsed_ms: u64,
    source_change_token_requests: u64,
    source_change_token_observations: u64,
    source_snapshot_requests: u64,
    source_snapshot_node_reads: u64,
    source_snapshot_bytes: u64,
    embedding_cache_hits: u64,
    embedding_inputs: u64,
    embedding_input_bytes: u64,
    provider_requests: u64,
    provider_inputs: u64,
    provider_input_bytes: u64,
    publication_attempts: u64,
    publication_records: u64,
}

impl SemanticRefreshRunMetrics {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn outcome(&self) -> SemanticRefreshRunOutcome {
        self.outcome
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// Repository change-token calls started by this refresh attempt.
    pub fn source_change_token_requests(&self) -> u64 {
        self.source_change_token_requests
    }

    /// Valid `Some` change tokens returned by those repository calls.
    pub fn source_change_token_observations(&self) -> u64 {
        self.source_change_token_observations
    }

    pub fn source_snapshot_requests(&self) -> u64 {
        self.source_snapshot_requests
    }

    pub fn source_snapshot_node_reads(&self) -> u64 {
        self.source_snapshot_node_reads
    }

    pub fn source_snapshot_bytes(&self) -> u64 {
        self.source_snapshot_bytes
    }

    pub fn embedding_cache_hits(&self) -> u64 {
        self.embedding_cache_hits
    }

    pub fn embedding_inputs(&self) -> u64 {
        self.embedding_inputs
    }

    pub fn embedding_input_bytes(&self) -> u64 {
        self.embedding_input_bytes
    }

    /// Provider-adapter invocations, including retries and failed invocations.
    ///
    /// This counter records work at Code's adapter boundary. It does not prove
    /// that an adapter transmitted a remote request or incurred a charge.
    pub fn provider_requests(&self) -> u64 {
        self.provider_requests
    }

    /// Inputs offered to provider-adapter invocations; retries count again.
    pub fn provider_inputs(&self) -> u64 {
        self.provider_inputs
    }

    /// UTF-8 input bytes offered to provider-adapter invocations, including
    /// retries. This is not proof of remote transmission or billing.
    pub fn provider_input_bytes(&self) -> u64 {
        self.provider_input_bytes
    }

    /// Complete-partition publication calls that reached the index backend.
    ///
    /// A rejected CAS publication counts. Later invalidation cleanup does not.
    pub fn publication_attempts(&self) -> u64 {
        self.publication_attempts
    }

    /// Records offered by complete-partition publication attempts.
    ///
    /// A rejected CAS publication counts. Later invalidation cleanup does not.
    pub fn publication_records(&self) -> u64 {
        self.publication_records
    }
}

/// Bounded cumulative metrics for one semantic refresh ownership epoch.
///
/// Counts, byte sizes, outcomes, and elapsed time are retained. Source text,
/// node identifiers, digests, vectors, provider identities, and error bodies
/// are deliberately absent.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRefreshMetrics {
    ownership_epoch: u64,
    attempted_runs: u64,
    published_runs: u64,
    unchanged_runs: u64,
    failed_runs: u64,
    total_elapsed_ms: u64,
    max_elapsed_ms: u64,
    total_source_change_token_requests: u64,
    total_source_change_token_observations: u64,
    total_source_snapshot_requests: u64,
    total_source_snapshot_node_reads: u64,
    total_source_snapshot_bytes: u64,
    total_embedding_cache_hits: u64,
    total_embedding_inputs: u64,
    total_embedding_input_bytes: u64,
    total_provider_requests: u64,
    total_provider_inputs: u64,
    total_provider_input_bytes: u64,
    total_publication_attempts: u64,
    total_publication_records: u64,
    recent_runs: Vec<SemanticRefreshRunMetrics>,
}

impl SemanticRefreshMetrics {
    pub fn ownership_epoch(&self) -> u64 {
        self.ownership_epoch
    }

    /// Attempts that returned a terminal result.
    ///
    /// A force-aborted future cannot contribute because it never settled.
    pub fn attempted_runs(&self) -> u64 {
        self.attempted_runs
    }

    pub fn published_runs(&self) -> u64 {
        self.published_runs
    }

    pub fn unchanged_runs(&self) -> u64 {
        self.unchanged_runs
    }

    pub fn failed_runs(&self) -> u64 {
        self.failed_runs
    }

    pub fn total_elapsed_ms(&self) -> u64 {
        self.total_elapsed_ms
    }

    pub fn max_elapsed_ms(&self) -> u64 {
        self.max_elapsed_ms
    }

    pub fn total_source_change_token_requests(&self) -> u64 {
        self.total_source_change_token_requests
    }

    pub fn total_source_change_token_observations(&self) -> u64 {
        self.total_source_change_token_observations
    }

    pub fn total_source_snapshot_requests(&self) -> u64 {
        self.total_source_snapshot_requests
    }

    pub fn total_source_snapshot_node_reads(&self) -> u64 {
        self.total_source_snapshot_node_reads
    }

    pub fn total_source_snapshot_bytes(&self) -> u64 {
        self.total_source_snapshot_bytes
    }

    pub fn total_embedding_cache_hits(&self) -> u64 {
        self.total_embedding_cache_hits
    }

    pub fn total_embedding_inputs(&self) -> u64 {
        self.total_embedding_inputs
    }

    pub fn total_embedding_input_bytes(&self) -> u64 {
        self.total_embedding_input_bytes
    }

    /// Provider-adapter invocations, including retries and failed invocations.
    ///
    /// This counter records work at Code's adapter boundary. It does not prove
    /// that an adapter transmitted a remote request or incurred a charge.
    pub fn total_provider_requests(&self) -> u64 {
        self.total_provider_requests
    }

    /// Inputs offered to provider-adapter invocations; retries count again.
    pub fn total_provider_inputs(&self) -> u64 {
        self.total_provider_inputs
    }

    /// UTF-8 input bytes offered to provider-adapter invocations, including
    /// retries. This is not proof of remote transmission or billing.
    pub fn total_provider_input_bytes(&self) -> u64 {
        self.total_provider_input_bytes
    }

    /// Complete-partition publication calls that reached the index backend.
    ///
    /// Rejected CAS publications count. Later invalidation cleanup does not.
    pub fn total_publication_attempts(&self) -> u64 {
        self.total_publication_attempts
    }

    /// Records offered by complete-partition publication attempts.
    ///
    /// Rejected CAS publications count. Later invalidation cleanup does not.
    pub fn total_publication_records(&self) -> u64 {
        self.total_publication_records
    }

    pub fn recent_runs(&self) -> &[SemanticRefreshRunMetrics] {
        &self.recent_runs
    }

    pub fn last_run(&self) -> Option<&SemanticRefreshRunMetrics> {
        self.recent_runs.last()
    }

    pub(super) fn for_epoch(ownership_epoch: u64) -> Self {
        Self {
            ownership_epoch,
            ..Self::default()
        }
    }

    pub(super) fn record(&mut self, observation: SemanticRefreshRunObservation) {
        self.attempted_runs = self.attempted_runs.saturating_add(1);
        match observation.outcome {
            SemanticRefreshRunOutcome::Published => {
                self.published_runs = self.published_runs.saturating_add(1);
            }
            SemanticRefreshRunOutcome::Unchanged => {
                self.unchanged_runs = self.unchanged_runs.saturating_add(1);
            }
            SemanticRefreshRunOutcome::Failed => {
                self.failed_runs = self.failed_runs.saturating_add(1);
            }
        }
        let elapsed_ms = duration_ms(observation.elapsed);
        self.total_elapsed_ms = self.total_elapsed_ms.saturating_add(elapsed_ms);
        self.max_elapsed_ms = self.max_elapsed_ms.max(elapsed_ms);
        self.total_source_change_token_requests = self
            .total_source_change_token_requests
            .saturating_add(as_u64(observation.source_change_token_requests));
        self.total_source_change_token_observations = self
            .total_source_change_token_observations
            .saturating_add(as_u64(observation.source_change_token_observations));
        self.total_source_snapshot_requests = self
            .total_source_snapshot_requests
            .saturating_add(as_u64(observation.source_snapshot_requests));
        self.total_source_snapshot_node_reads = self
            .total_source_snapshot_node_reads
            .saturating_add(as_u64(observation.source_snapshot_node_reads));
        self.total_source_snapshot_bytes = self
            .total_source_snapshot_bytes
            .saturating_add(as_u64(observation.source_snapshot_bytes));
        self.total_embedding_cache_hits = self
            .total_embedding_cache_hits
            .saturating_add(as_u64(observation.embedding_cache_hits));
        self.total_embedding_inputs = self
            .total_embedding_inputs
            .saturating_add(as_u64(observation.embedding_inputs));
        self.total_embedding_input_bytes = self
            .total_embedding_input_bytes
            .saturating_add(as_u64(observation.embedding_input_bytes));
        self.total_provider_requests = self
            .total_provider_requests
            .saturating_add(as_u64(observation.provider_requests));
        self.total_provider_inputs = self
            .total_provider_inputs
            .saturating_add(as_u64(observation.provider_inputs));
        self.total_provider_input_bytes = self
            .total_provider_input_bytes
            .saturating_add(as_u64(observation.provider_input_bytes));
        self.total_publication_attempts = self
            .total_publication_attempts
            .saturating_add(as_u64(observation.publication_attempts));
        self.total_publication_records = self
            .total_publication_records
            .saturating_add(as_u64(observation.publication_records));

        if self.recent_runs.len() == SEMANTIC_REFRESH_RECENT_RUN_LIMIT {
            self.recent_runs.remove(0);
        }
        self.recent_runs.push(SemanticRefreshRunMetrics {
            sequence: self.attempted_runs,
            outcome: observation.outcome,
            elapsed_ms,
            source_change_token_requests: as_u64(observation.source_change_token_requests),
            source_change_token_observations: as_u64(observation.source_change_token_observations),
            source_snapshot_requests: as_u64(observation.source_snapshot_requests),
            source_snapshot_node_reads: as_u64(observation.source_snapshot_node_reads),
            source_snapshot_bytes: as_u64(observation.source_snapshot_bytes),
            embedding_cache_hits: as_u64(observation.embedding_cache_hits),
            embedding_inputs: as_u64(observation.embedding_inputs),
            embedding_input_bytes: as_u64(observation.embedding_input_bytes),
            provider_requests: as_u64(observation.provider_requests),
            provider_inputs: as_u64(observation.provider_inputs),
            provider_input_bytes: as_u64(observation.provider_input_bytes),
            publication_attempts: as_u64(observation.publication_attempts),
            publication_records: as_u64(observation.publication_records),
        });
    }
}

pub(super) struct SemanticRefreshRunObservation {
    pub(super) outcome: SemanticRefreshRunOutcome,
    pub(super) elapsed: Duration,
    pub(super) source_change_token_requests: usize,
    pub(super) source_change_token_observations: usize,
    pub(super) source_snapshot_requests: usize,
    pub(super) source_snapshot_node_reads: usize,
    pub(super) source_snapshot_bytes: usize,
    pub(super) embedding_cache_hits: usize,
    pub(super) embedding_inputs: usize,
    pub(super) embedding_input_bytes: usize,
    pub(super) provider_requests: usize,
    pub(super) provider_inputs: usize,
    pub(super) provider_input_bytes: usize,
    pub(super) publication_attempts: usize,
    pub(super) publication_records: usize,
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(outcome: SemanticRefreshRunOutcome) -> SemanticRefreshRunObservation {
        SemanticRefreshRunObservation {
            outcome,
            elapsed: Duration::from_millis(3),
            source_change_token_requests: 3,
            source_change_token_observations: 3,
            source_snapshot_requests: 2,
            source_snapshot_node_reads: 4,
            source_snapshot_bytes: 128,
            embedding_cache_hits: 1,
            embedding_inputs: 1,
            embedding_input_bytes: 64,
            provider_requests: 2,
            provider_inputs: 2,
            provider_input_bytes: 128,
            publication_attempts: 1,
            publication_records: 2,
        }
    }

    #[test]
    fn cumulative_metrics_saturate_and_recent_runs_are_strictly_bounded() {
        let mut metrics = SemanticRefreshMetrics::for_epoch(7);
        for _ in 0..SEMANTIC_REFRESH_RECENT_RUN_LIMIT + 3 {
            metrics.record(observation(SemanticRefreshRunOutcome::Published));
        }
        assert_eq!(metrics.ownership_epoch(), 7);
        assert_eq!(metrics.attempted_runs(), 67);
        assert_eq!(metrics.published_runs(), 67);
        assert_eq!(metrics.total_source_change_token_requests(), 201);
        assert_eq!(metrics.total_source_change_token_observations(), 201);
        assert_eq!(metrics.total_provider_requests(), 134);
        assert_eq!(metrics.total_provider_inputs(), 134);
        assert_eq!(metrics.total_provider_input_bytes(), 8_576);
        assert_eq!(metrics.recent_runs().len(), 64);
        assert_eq!(metrics.recent_runs()[0].sequence(), 4);
        assert_eq!(metrics.last_run().unwrap().sequence(), 67);
        assert_eq!(metrics.total_elapsed_ms(), 201);
        assert_eq!(metrics.max_elapsed_ms(), 3);

        let mut saturated = SemanticRefreshMetrics::for_epoch(u64::MAX);
        saturated.attempted_runs = u64::MAX;
        saturated.total_elapsed_ms = u64::MAX;
        saturated.total_provider_input_bytes = u64::MAX;
        saturated.record(observation(SemanticRefreshRunOutcome::Failed));
        assert_eq!(saturated.attempted_runs(), u64::MAX);
        assert_eq!(saturated.total_elapsed_ms(), u64::MAX);
        assert_eq!(saturated.total_provider_input_bytes(), u64::MAX);
        assert_eq!(saturated.last_run().unwrap().sequence(), u64::MAX);
    }
}
