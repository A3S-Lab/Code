//! Shared-index revision CAS under concurrent multi-agent writers.
//!
//! Two independent proofs run against one Redis keyspace:
//!
//! 1. A racing probe where every writer captures the same expected revision.
//!    Exactly one may publish; the rest must be refused with a revision
//!    conflict rather than replacing a newer generation.
//! 2. A convergence loop where each writer re-observes and retries until it
//!    lands, proving the index still advances one revision per publication and
//!    loses no writer's partition.
//!
//! Every writer owns its own Redis socket, so the ordering comes from the
//! server's linearization point instead of in-process serialization.

use super::connection::RedisSocket;
use super::redis_index::RedisVectorIndex;
use a3s_memory::vector::{
    VectorIndex, VectorIndexDescriptor, VectorIndexError, VectorRecord, VectorRevision,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::Arc;

/// Outcome of the concurrent-writer qualification.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyEvidence {
    pub writers: usize,
    pub independent_sockets: usize,
    pub mutation_consistency: String,
    pub race_committed: usize,
    pub race_conflicted: usize,
    pub race_other_errors: Vec<String>,
    pub revision_before: u64,
    pub revision_after_race: u64,
    pub revision_after_convergence: u64,
    pub convergence_retries: u64,
    pub partitions_after_convergence: usize,
    pub records_after_convergence: usize,
    /// One publication advanced the revision by exactly one.
    pub race_advanced_by_one: bool,
    /// Every writer's partition survived the convergence loop.
    pub all_writers_landed: bool,
    pub cas_conflicts_observed: u64,
    pub cas_aborts_observed: u64,
    pub passed: bool,
}

/// Run the racing and converging writer probes against `prefix`.
pub async fn qualify(
    redis_url: &str,
    prefix: &str,
    descriptor: VectorIndexDescriptor,
    writers: usize,
    embedding: Vec<f32>,
) -> Result<ConcurrencyEvidence> {
    let mut handles = Vec::with_capacity(writers);
    for _ in 0..writers {
        let socket = RedisSocket::connect(redis_url)
            .await
            .context("could not dial an independent writer socket")?;
        handles.push(Arc::new(
            RedisVectorIndex::open(socket, prefix, descriptor.clone()).await?,
        ));
    }
    let coordinator = handles
        .first()
        .cloned()
        .context("at least one writer is required")?;
    let revision_before = coordinator.observe().await?.status.revision;

    let mut race = Vec::with_capacity(writers);
    for (writer, index) in handles.iter().enumerate() {
        let index = Arc::clone(index);
        let records = vec![writer_record(writer, &embedding)];
        let partition = writer_partition(prefix, writer);
        race.push(tokio::spawn(async move {
            index
                .replace_partition_if_revision(&partition, revision_before, records)
                .await
        }));
    }

    let mut race_committed = 0usize;
    let mut race_conflicted = 0usize;
    let mut race_other_errors = Vec::new();
    for handle in race {
        match handle.await.context("writer task panicked")? {
            Ok(_) => race_committed += 1,
            Err(VectorIndexError::RevisionConflict { .. }) => race_conflicted += 1,
            Err(error) => race_other_errors.push(error.to_string()),
        }
    }
    let revision_after_race = coordinator.observe().await?.status.revision;

    // Convergence: every writer retries against the revision it just observed
    // until its own partition is published.
    let mut convergence = Vec::with_capacity(writers);
    for (writer, index) in handles.iter().enumerate() {
        let index = Arc::clone(index);
        let records = vec![writer_record(writer, &embedding)];
        let partition = writer_partition(prefix, writer);
        convergence.push(tokio::spawn(async move {
            let mut retries = 0u64;
            loop {
                let expected = index.observe().await?.status.revision;
                match index
                    .replace_partition_if_revision(&partition, expected, records.clone())
                    .await
                {
                    Ok(status) => {
                        return Ok::<(u64, VectorRevision), VectorIndexError>((
                            retries,
                            status.revision,
                        ))
                    }
                    Err(VectorIndexError::RevisionConflict { .. }) => {
                        retries = retries.saturating_add(1);
                        if retries > 256 {
                            return Err(VectorIndexError::StorageFailed(
                                "writer never converged".into(),
                            ));
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }));
    }

    let mut convergence_retries = 0u64;
    let mut landed = 0usize;
    for handle in convergence {
        let (retries, _) = handle.await.context("writer task panicked?")??;
        convergence_retries = convergence_retries.saturating_add(retries);
        landed += 1;
    }

    let after = coordinator.observe().await?;
    let cas_conflicts_observed = handles.iter().map(|index| index.cas_conflicts()).sum();
    let cas_aborts_observed = handles.iter().map(|index| index.cas_aborts()).sum();

    // The racing probe writes one partition; convergence publishes the
    // remaining `writers - 1` plus a no-op-free rewrite of the winner's own.
    let all_writers_landed = landed == writers && after.status.partition_count == writers;
    let race_advanced_by_one =
        revision_after_race.value() == revision_before.value().saturating_add(1);
    let passed = race_committed == 1
        && race_conflicted == writers - 1
        && race_other_errors.is_empty()
        && race_advanced_by_one
        && all_writers_landed
        && after.status.record_count == writers;

    let evidence = ConcurrencyEvidence {
        writers,
        independent_sockets: writers,
        mutation_consistency: format!("{:?}", coordinator.mutation_consistency()),
        race_committed,
        race_conflicted,
        race_other_errors,
        revision_before: revision_before.value(),
        revision_after_race: revision_after_race.value(),
        revision_after_convergence: after.status.revision.value(),
        convergence_retries,
        partitions_after_convergence: after.status.partition_count,
        records_after_convergence: after.status.record_count,
        race_advanced_by_one,
        all_writers_landed,
        cas_conflicts_observed,
        cas_aborts_observed,
        passed,
    };

    coordinator.destroy_keyspace().await?;
    Ok(evidence)
}

fn writer_partition(prefix: &str, writer: usize) -> String {
    format!("{prefix}-writer-{writer:03}")
}

fn writer_record(writer: usize, embedding: &[f32]) -> VectorRecord {
    VectorRecord::new(format!("writer-record-{writer:03}"), embedding.to_vec())
        .with_label("a3s.dm-prod1.writer", writer.to_string())
}
