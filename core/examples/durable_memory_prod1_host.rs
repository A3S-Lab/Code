//! `DM-PROD1` (`HARNESS-CONV7`) host qualification harness.
//!
//! Produces a reproducible host report pack for the production-qualification
//! overlay in `manual/DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md`. Unlike the
//! hermetic `DURABLE_MEMORY_*` gates, every backend here is real: a Boyue
//! OpenAI-compatible embedding endpoint, a Redis-backed `VectorIndex` with
//! index-revision CAS, and a Redis `SET NX EX` lease.
//!
//! Run through `scripts/harbor/run_dm_prod1_host.sh`, which sources credentials
//! from `scripts/harbor/.env`. The harness never writes credentials or memory
//! plaintext into the pack and fails the `secret_hygiene` row if it finds any.

#[path = "durable_memory_prod1_host/boyue.rs"]
mod boyue;
#[path = "durable_memory_prod1_host/concurrency.rs"]
mod concurrency;
#[path = "durable_memory_prod1_host/config.rs"]
mod config;
#[path = "durable_memory_prod1_host/connection.rs"]
mod connection;
#[path = "durable_memory_prod1_host/corpus.rs"]
mod corpus;
#[path = "durable_memory_prod1_host/horizons.rs"]
mod horizons;
#[path = "durable_memory_prod1_host/lease.rs"]
mod lease;
#[path = "durable_memory_prod1_host/redis_index.rs"]
mod redis_index;
#[path = "durable_memory_prod1_host/redis_store.rs"]
mod redis_store;
#[path = "durable_memory_prod1_host/report.rs"]
mod report;
#[path = "durable_memory_prod1_host/resilience.rs"]
mod resilience;
#[path = "durable_memory_prod1_host/session.rs"]
mod session;

use a3s_code_core::embedding::EmbeddingProvider;
use a3s_code_core::memory::{ScheduledSemanticRefresh, SemanticRefreshRunOutcome};
use a3s_memory::repository::{FileMemoryRepository, MemoryNamespace, MemoryRepository};
use a3s_memory::vector::{VectorIndex, VectorIndexDescriptor};
use anyhow::{Context, Result};
use boyue::BoyueEmbeddingProvider;
use concurrency::ConcurrencyEvidence;
use connection::RedisSocket;
use horizons::{HorizonContext, HorizonRecord};
use lease::{LeaseAcquisition, RedisLeasePolicy};
use redis_index::RedisVectorIndex;
use report::{Dimension, Distribution, HygienePolicy};
use resilience::{FailoverEvidence, RestartRecord};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const ACTIVE_SEED_NODES: usize = 48;
const CANDIDATE_SEED_NODES: usize = 8;
const ACTIVATED_CANDIDATES: usize = 4;
const CONSOLIDATED_NODES: usize = 2;
const PEER_AGENT_NODES: usize = 8;
const RESTART_CYCLES: usize = 2;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const NODE_LIMIT: usize = 256;
const VECTOR_MAX_BYTES: usize = 64 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::HostConfig::from_environment()?;
    eprintln!(
        "dm-prod1: redis={} model={} host={} revision={}",
        config.redacted_redis_target(),
        config.endpoint.model,
        config.endpoint.host(),
        config.revision
    );

    // ---- Real embedding provider -------------------------------------------
    let probe_started = Instant::now();
    let provider = Arc::new(
        BoyueEmbeddingProvider::probe(config.endpoint.clone(), config.api_key.clone()).await?,
    );
    let probe_ms = probe_started.elapsed().as_secs_f64() * 1_000.0;
    let dimension = provider.dimension();
    let descriptor = VectorIndexDescriptor::new(dimension)
        .with_max_records(NODE_LIMIT * 4)
        .with_max_bytes(VECTOR_MAX_BYTES);
    let probe_vector = enumeration_probe(dimension);

    // ---- Durable remote backend --------------------------------------------
    let index_prefix = format!("{}:index", config.key_prefix);
    let admin_socket = RedisSocket::connect(&config.redis_url)
        .await
        .context("could not dial the Redis administration socket")?;
    let index_socket = RedisSocket::connect(&config.redis_url)
        .await
        .context("could not dial the Redis index socket")?;
    let index = Arc::new(
        RedisVectorIndex::open(Arc::clone(&index_socket), &index_prefix, descriptor.clone())
            .await?,
    );
    let original_history = index.history_digest().to_string();

    // ---- Source repository and corpus --------------------------------------
    let root = tempfile::tempdir().context("could not create the qualification directory")?;
    let source_root = root.path().join("source");
    let namespace = MemoryNamespace::try_new(
        "dm-prod1-tenant",
        "dm-prod1-principal",
        "dm-prod1-scope-primary",
    )?;
    let peer_namespace = MemoryNamespace::try_new(
        "dm-prod1-tenant",
        "dm-prod1-principal",
        "dm-prod1-scope-peer",
    )?;
    let repository = Arc::new(FileMemoryRepository::open(&source_root).await?);
    let seeded = corpus::seed(
        repository.as_ref(),
        &namespace,
        ACTIVE_SEED_NODES,
        CANDIDATE_SEED_NODES,
    )
    .await?;
    let peer_seeded =
        corpus::seed(repository.as_ref(), &peer_namespace, PEER_AGENT_NODES, 0).await?;

    // ---- Distributed lease --------------------------------------------------
    let lease_policy = RedisLeasePolicy::new(
        Arc::clone(&admin_socket),
        &config.key_prefix,
        config.lease_ttl,
    );
    let primary_grant = match lease_policy.acquire("dm-prod1-owner-a").await? {
        LeaseAcquisition::Granted(grant) => grant,
        LeaseAcquisition::Held { fence } => {
            anyhow::bail!("a fresh lease key was already held (fence {fence:?})")
        }
    };
    let contended = lease_policy.acquire("dm-prod1-owner-b").await?;
    let lease_excludes_second_owner = matches!(contended, LeaseAcquisition::Held { .. });
    let lease_ttl_seconds = lease_policy.ttl_seconds().await?;
    let lease_held_by_owner = lease_policy.holds(&primary_grant).await?;

    // ---- Horizons -----------------------------------------------------------
    let authority = session::authority_digest(&config.revision);
    let dyn_provider: Arc<dyn EmbeddingProvider> = provider.clone();
    let dyn_repository: Arc<dyn MemoryRepository> = repository.clone();
    let dyn_index: Arc<dyn VectorIndex> = index.clone();
    let durable = session::durable_session(
        dyn_repository.clone(),
        namespace.clone(),
        dyn_provider.clone(),
        dyn_index.clone(),
        &authority,
        NODE_LIMIT,
    )?;
    let first_binding_digest = resilience::binding_digest(&durable.binding())?;
    let first_serving_generation = durable
        .semantic_recall()
        .context("semantic recall was not attached")?
        .binding()
        .authority_digest()
        .to_string();

    let schedule = ScheduledSemanticRefresh::try_new(REFRESH_INTERVAL)?;
    let runtime = session::start_runtime("dm-prod1-owner-a", durable.clone(), schedule.clone())?;
    let horizon_context = HorizonContext {
        session: &durable,
        schedule: &schedule,
        repository: dyn_repository.clone(),
        index: dyn_index.clone(),
        query_vector: probe_vector.clone(),
        target_node_id: seeded.target_id.clone(),
        node_limit: NODE_LIMIT,
    };

    let mut horizon_records: Vec<HorizonRecord> = Vec::new();
    let first_run = session::wait_for_run(&schedule, 1).await?;
    horizon_records
        .push(horizons::settle(&horizon_context, "initial_publication", first_run).await?);

    let baseline = session::attempted_runs(&schedule)?;
    horizons::activate_candidates(&durable, &seeded.candidate_ids, ACTIVATED_CANDIDATES, 1).await?;
    horizon_records.push(
        horizons::observe_publication(&horizon_context, "candidate_activation", baseline).await?,
    );

    let baseline = session::attempted_runs(&schedule)?;
    corpus::revise(repository.as_ref(), &namespace, 2).await?;
    horizon_records.push(
        horizons::observe_publication(&horizon_context, "single_node_drift", baseline).await?,
    );

    // Fold the newest Active nodes into the oldest ones: a real consolidation
    // keeps a survivor, so the report can prove the Active projection shrank
    // without any node being silently dropped.
    let consolidated: Vec<(String, String)> = seeded
        .active_ids
        .iter()
        .rev()
        .take(CONSOLIDATED_NODES)
        .cloned()
        .zip(seeded.active_ids.iter().take(CONSOLIDATED_NODES).cloned())
        .collect();
    let baseline = session::attempted_runs(&schedule)?;
    corpus::supersede(repository.as_ref(), &namespace, &consolidated, 3).await?;
    horizon_records.push(
        horizons::observe_publication(&horizon_context, "consolidation_decay", baseline).await?,
    );

    // Nothing mutates here, so this horizon measures the no-op fast path.
    let baseline = session::attempted_runs(&schedule)?;
    horizon_records
        .push(horizons::observe_quiescent(&horizon_context, "steady_state_cache", baseline).await?);

    let epoch_metrics = schedule.metrics();
    let published_observation = index.observe().await?;
    let first_close = runtime.close().await;
    drop(runtime);

    // ---- Peer agent on the same shared index --------------------------------
    let peer = session::durable_session(
        dyn_repository.clone(),
        peer_namespace.clone(),
        dyn_provider.clone(),
        dyn_index.clone(),
        &authority,
        NODE_LIMIT,
    )?;
    peer.refresh_semantic_recall_requiring(
        a3s_memory::vector::VectorMutationConsistency::IndexRevisionCas,
        CancellationToken::new(),
    )
    .await?;
    let shared = index.observe().await?;
    let primary_preview = durable.preview_recall(corpus::TARGET_QUERY).await?;
    let peer_preview = peer.preview_recall(corpus::TARGET_QUERY).await?;
    let primary_ids: Vec<String> = primary_preview
        .hits
        .iter()
        .map(|hit| hit.node_id.clone())
        .collect();
    let peer_ids: Vec<String> = peer_preview
        .hits
        .iter()
        .map(|hit| hit.node_id.clone())
        .collect();
    let peer_node_set: std::collections::BTreeSet<&String> =
        peer_seeded.active_ids.iter().collect();
    let primary_node_set: std::collections::BTreeSet<&String> = seeded
        .active_ids
        .iter()
        .chain(seeded.candidate_ids.iter())
        .collect();
    // A shared index must not let one agent's recall reach another agent's
    // namespace. Count the crossings instead of collapsing them into one bool so
    // a failure names which direction leaked.
    let primary_foreign_hits = primary_ids
        .iter()
        .filter(|id| peer_node_set.contains(id))
        .count();
    let peer_foreign_hits = peer_ids
        .iter()
        .filter(|id| primary_node_set.contains(id))
        .count();
    let namespace_isolation = shared.status.partition_count == 2
        && primary_foreign_hits == 0
        && peer_foreign_hits == 0
        && !peer_ids.is_empty()
        && !primary_ids.is_empty();
    drop(peer);

    // The peer published into the shared index, so the primary's horizon-epoch
    // receipt no longer names the live index revision. Re-settle the primary and
    // checkpoint the quiescent generation: that is the state a restart has to
    // resume exactly.
    let quiescent_schedule = ScheduledSemanticRefresh::try_new(REFRESH_INTERVAL)?;
    let quiescent_runtime = session::start_runtime(
        "dm-prod1-owner-a-quiescent",
        durable.clone(),
        quiescent_schedule.clone(),
    )?;
    let quiescent_run = session::wait_for_unchanged_run(&quiescent_schedule, 0).await?;
    let receipt = quiescent_schedule
        .last_receipt()
        .context("the quiescent ownership epoch retained no receipt")?;
    let checkpoint = receipt.checkpoint();
    let checkpoint_json = serde_json::to_vec(&checkpoint)?;
    let quiescent_close = quiescent_runtime.close().await;
    drop(quiescent_runtime);

    // ---- Repeated restart ---------------------------------------------------
    // `FileMemoryRepository` holds an exclusive advisory lock on its directory,
    // so a restart is only a real restart once every handle from the previous
    // epoch is gone. These drops release the trait-object clones the horizon
    // phase kept alive.
    drop(horizon_context);
    drop(dyn_repository);
    drop(dyn_index);

    let mut restart_records: Vec<RestartRecord> = Vec::new();
    let mut durable = durable;
    let mut repository = repository;
    let mut index = index;
    let mut index_socket = index_socket;
    for cycle in 1..=RESTART_CYCLES {
        let before = index.observe().await?;
        drop(durable);
        drop(index);
        drop(repository);
        drop(index_socket);

        let started = Instant::now();
        index_socket = RedisSocket::connect(&config.redis_url).await?;
        repository = Arc::new(FileMemoryRepository::open(&source_root).await?);
        index = Arc::new(
            RedisVectorIndex::open(Arc::clone(&index_socket), &index_prefix, descriptor.clone())
                .await?,
        );
        let dyn_repository: Arc<dyn MemoryRepository> = repository.clone();
        let dyn_index: Arc<dyn VectorIndex> = index.clone();
        durable = session::durable_session(
            dyn_repository,
            namespace.clone(),
            dyn_provider.clone(),
            dyn_index,
            &authority,
            NODE_LIMIT,
        )?;
        let reopen_ms = started.elapsed().as_millis() as u64;
        let after = index.observe().await?;
        let preview = durable.preview_recall(corpus::TARGET_QUERY).await?;

        restart_records.push(RestartRecord {
            cycle,
            history_digest_stable: index.history_digest() == original_history,
            revision: after.status.revision.value(),
            revision_preserved: after.status.revision >= before.status.revision,
            record_count: after.status.record_count,
            records_preserved: after.status.record_count == before.status.record_count,
            binding_digest_stable: resilience::binding_digest(&durable.binding())?
                == first_binding_digest,
            serving_generation_stable: durable
                .semantic_recall()
                .map(|semantic| semantic.binding().authority_digest() == first_serving_generation)
                .unwrap_or(false),
            target_recall_rank: preview
                .hits
                .iter()
                .position(|hit| hit.node_id == seeded.target_id),
            reopen_ms,
        });
    }

    // A checkpoint-recovered schedule must adopt the persisted receipt and
    // settle its first run as Unchanged, which is the exact-resume proof.
    let decoded: a3s_code_core::DurableMemorySemanticRefreshCheckpoint =
        serde_json::from_slice(&checkpoint_json)?;
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(REFRESH_INTERVAL, decoded.clone())?;
    let recovered_runtime = session::start_runtime(
        "dm-prod1-owner-a-recovered",
        durable.clone(),
        recovered_schedule.clone(),
    )?;
    let recovered_run = session::wait_for_run(&recovered_schedule, 1).await?;
    let recovered_close = recovered_runtime.close().await;
    drop(recovered_runtime);
    let checkpoint_resume_unchanged = recovered_run.outcome()
        == SemanticRefreshRunOutcome::Unchanged
        && recovered_run.provider_requests() == 0
        && decoded == checkpoint;

    // ---- Failover -----------------------------------------------------------
    let before_failover = index.observe().await?;
    let killed_clients = resilience::kill_other_clients(&admin_socket).await?;
    let after_failover = index.observe().await;
    let failover_recall = match &after_failover {
        Ok(_) => durable.preview_recall(corpus::TARGET_QUERY).await.ok(),
        Err(_) => None,
    };
    let alternate_prefix = format!("{}:alternate", config.key_prefix);
    let (alternate_history_rotated, alternate_revision_reset) =
        resilience::rotate_alternate_keyspace(
            &config.redis_url,
            &alternate_prefix,
            descriptor.clone(),
        )
        .await?;
    let failover = FailoverEvidence {
        killed_clients,
        reconnects: index.reconnects(),
        sockets_dialled: index_socket.dials(),
        survived_connection_drop: after_failover.is_ok(),
        history_digest_stable_after_drop: index.history_digest() == original_history,
        revision_preserved_after_drop: after_failover
            .as_ref()
            .map(|observation| observation.status.revision >= before_failover.status.revision)
            .unwrap_or(false),
        records_preserved_after_drop: after_failover
            .as_ref()
            .map(|observation| {
                observation.status.record_count == before_failover.status.record_count
            })
            .unwrap_or(false),
        recall_rank_after_drop: failover_recall.as_ref().and_then(|preview| {
            preview
                .hits
                .iter()
                .position(|hit| hit.node_id == seeded.target_id)
        }),
        alternate_history_rotated,
        alternate_revision_reset,
        primary_history_unaffected: index.history_digest() == original_history,
        passed: false,
    };
    let failover = FailoverEvidence {
        passed: failover.survived_connection_drop
            && failover.reconnects > 0
            && failover.history_digest_stable_after_drop
            && failover.revision_preserved_after_drop
            && failover.records_preserved_after_drop
            && failover.recall_rank_after_drop.is_some()
            && failover.alternate_history_rotated
            && failover.alternate_revision_reset,
        ..failover
    };

    // ---- Concurrent multi-agent writers ------------------------------------
    let concurrency: ConcurrencyEvidence = concurrency::qualify(
        &config.redis_url,
        &format!("{}:shared", config.key_prefix),
        descriptor.clone(),
        config.concurrent_writers,
        probe_vector.clone(),
    )
    .await?;

    // ---- Lease fencing and release -----------------------------------------
    // The epoch lease is never renewed by this harness, so whether it outlived
    // the whole run is evidence rather than a gate. Release, fencing, and
    // stale-owner refusal are proven on a fresh grant, which keeps the verdict
    // independent of how long the embedding provider took.
    let epoch_lease_held_at_close = lease_policy.holds(&primary_grant).await?;
    let epoch_lease_ttl_at_close = lease_policy.ttl_seconds().await?;
    lease_policy.force_expire().await?;

    let lifecycle_grant = match lease_policy.acquire("dm-prod1-owner-c").await? {
        LeaseAcquisition::Granted(grant) => grant,
        LeaseAcquisition::Held { fence } => {
            anyhow::bail!("a deleted lease key was still held (fence {fence:?})")
        }
    };
    let released = lease_policy.release(&lifecycle_grant).await?;
    let successor_grant = match lease_policy.acquire("dm-prod1-owner-d").await? {
        LeaseAcquisition::Granted(grant) => Some(grant),
        LeaseAcquisition::Held { .. } => None,
    };
    let fence_advanced = successor_grant
        .as_ref()
        .is_some_and(|grant| grant.fence > lifecycle_grant.fence);
    let stale_release_refused = !lease_policy.release(&lifecycle_grant).await?;
    if let Some(grant) = &successor_grant {
        lease_policy.release(grant).await?;
    }
    let lease_passed = lease_excludes_second_owner
        && lease_held_by_owner
        && lease_ttl_seconds.is_some_and(|ttl| ttl > 0)
        && released
        && fence_advanced
        && stale_release_refused;

    // ---- Provider distributions --------------------------------------------
    let telemetry = provider.telemetry();
    let latency = Distribution::from_samples(&telemetry.latencies_ms);
    let norms = Distribution::from_samples(&telemetry.vector_norms);
    let estimated_usd = provider.estimated_usd();
    let refresh_elapsed: Vec<f64> = horizon_records
        .iter()
        .map(|record| record.elapsed_ms as f64)
        .collect();

    // ---- Dimension verdicts -------------------------------------------------
    let horizons_passed = horizon_records.iter().all(HorizonRecord::passed);
    let restarts_passed = restart_records.iter().all(RestartRecord::passed)
        && restart_records.len() == RESTART_CYCLES
        && checkpoint_resume_unchanged;
    let cache_reuse_observed = horizon_records
        .iter()
        .any(|record| record.embedding_cache_hits > 0);
    let rebuild_observed = horizon_records
        .iter()
        .any(|record| record.provider_inputs > 0 && record.embedding_cache_hits == 0);
    let unchanged_observed = horizon_records
        .iter()
        .any(|record| record.outcome == SemanticRefreshRunOutcome::Unchanged);

    let dimensions = vec![
        Dimension::new(
            "long_horizon_consolidation_decay",
            "Long-horizon consolidation / decay",
            "Representative multi-session horizons; no silent Active-node loss",
            horizons_passed,
            json!({
                "horizons": horizon_records,
                "refreshElapsedMs": refresh_elapsed_distribution(&refresh_elapsed),
                "activatedCandidates": ACTIVATED_CANDIDATES,
                "consolidatedNodes": CONSOLIDATED_NODES,
                "epochMetrics": epoch_metrics,
                "runtimeCloseClean": first_close.is_clean()
                    && quiescent_close.is_clean()
                    && recovered_close.is_clean(),
            }),
        )
        .with_caveat(
            "Horizons are single-process and minutes-scale. Multi-day wall-clock \
             decay and cross-deployment session horizons are not covered.",
        ),
        Dimension::new(
            "real_embedding_provider",
            "Real embedding provider",
            "Exact provider/model identity recorded; billed-cost and latency distributions retained",
            telemetry.requests > 0 && telemetry.total_tokens > 0 && telemetry.failures == 0,
            json!({
                "provider": boyue::PROVIDER_ID,
                "model": config.endpoint.model,
                "endpointHost": config.endpoint.host(),
                "endpointDigest": config.endpoint.endpoint_digest(),
                "probedDimension": dimension,
                "probeLatencyMs": probe_ms,
                "descriptor": provider.descriptor(),
                "requests": telemetry.requests,
                "inputs": telemetry.inputs,
                "failures": telemetry.failures,
                "promptTokens": telemetry.prompt_tokens,
                "totalTokens": telemetry.total_tokens,
                "latencyMs": latency,
                "returnedVectorL2Norm": norms,
                "usdPerMillionTokens": config.endpoint.usd_per_million_tokens,
                "estimatedBilledUsd": estimated_usd,
            }),
        ),
        Dimension::new(
            "multi_agent_load",
            "Larger multi-agent load",
            "Shared-index revision-CAS holds under concurrent writers",
            concurrency.passed && namespace_isolation,
            json!({
                "concurrentWriters": concurrency,
                "sharedIndexNamespaceIsolation": {
                    "partitionCount": shared.status.partition_count,
                    "primaryRecallNodeCount": primary_ids.len(),
                    "peerRecallNodeCount": peer_ids.len(),
                    "primaryRecallForeignNodes": primary_foreign_hits,
                    "peerRecallForeignNodes": peer_foreign_hits,
                    "passed": namespace_isolation,
                },
            }),
        )
        .with_caveat(format!(
            "Load is {} concurrent writers over one Redis database in a single \
             process. Multi-host writer fleets are not covered.",
            config.concurrent_writers
        )),
        Dimension::new(
            "repeated_restart",
            "Repeated restart",
            "Exact binding resume; semantic generation identity stable",
            restarts_passed,
            json!({
                "cycles": restart_records,
                "bindingDigest": first_binding_digest,
                "indexHistoryDigest": original_history,
                "checkpointBytes": checkpoint_json.len(),
                "checkpointResumeUnchanged": checkpoint_resume_unchanged,
                "quiescentCheckpointSequence": quiescent_run.sequence(),
                "recoveredRunOutcome": format!("{:?}", recovered_run.outcome()),
                "publishedRecordCount": published_observation.status.record_count,
            }),
        ),
        Dimension::new(
            "durable_remote_cas_leases",
            "Durable remote CAS + leases",
            "Remote backend + distributed lease policy; failover exercised",
            failover.passed && lease_passed,
            json!({
                "vectorIndex": {
                    "backend": "example RedisVectorIndex (WATCH/MULTI/EXEC on the revision hash)",
                    "target": config.redacted_redis_target(),
                    "mutationConsistency": format!("{:?}", index.mutation_consistency()),
                    "historyDigest": original_history,
                    "revision": before_failover.status.revision.value(),
                },
                "leasePolicy": {
                    "mechanism": "Redis SET key value NX EX with INCR fence tokens",
                    "ttlSeconds": config.lease_ttl.as_secs(),
                    "observedTtlSeconds": lease_ttl_seconds,
                    "excludesSecondOwner": lease_excludes_second_owner,
                    "heldByOwner": lease_held_by_owner,
                    "ownerRelease": released,
                    "successorFenceAdvanced": fence_advanced,
                    "staleReleaseRefused": stale_release_refused,
                    "epochLeaseHeldAtClose": epoch_lease_held_at_close,
                    "epochLeaseTtlSecondsAtClose": epoch_lease_ttl_at_close,
                    "passed": lease_passed,
                },
                "failover": failover,
            }),
        )
        .with_caveat(
            "Failover is a client-side connection drop plus an independently \
             recreated keyspace. Redis Sentinel or Cluster primary promotion is \
             not exercised.",
        ),
        Dimension::new(
            "drift_cache",
            "Drift / cache",
            "Cache-hit vs rebuild distributions; no unauthorized namespace widening",
            cache_reuse_observed && rebuild_observed && unchanged_observed && namespace_isolation,
            json!({
                "perHorizon": horizon_records
                    .iter()
                    .map(|record| json!({
                        "label": record.label,
                        "outcome": record.outcome,
                        "embeddingCacheHits": record.embedding_cache_hits,
                        "providerRequests": record.provider_requests,
                        "providerInputs": record.provider_inputs,
                        "publicationRecords": record.publication_records,
                    }))
                    .collect::<Vec<_>>(),
                "cacheReuseObserved": cache_reuse_observed,
                "fullRebuildObserved": rebuild_observed,
                "unchangedFastPathObserved": unchanged_observed,
                "namespaceWideningDetected": !namespace_isolation,
            }),
        ),
    ];

    // ---- Pack ---------------------------------------------------------------
    let policy = HygienePolicy {
        secrets: vec![
            config.api_key.clone(),
            "sk-".to_string(),
            "Bearer ".to_string(),
            "BOYUE_API_KEY".to_string(),
        ],
        plaintext: corpus::plaintext_corpus(),
    };

    // Grade hygiene against the evidence that is about to be serialized, then
    // record that verdict as its own row. `write_pack` rescans the bytes on disk
    // afterwards, so a marker introduced by this row cannot slip through.
    let mut dimensions = dimensions;
    let evidence_findings = policy.scan_value("dimensions", &json!(dimensions))?;
    dimensions.push(Dimension::new(
        "secret_hygiene",
        "Secret hygiene",
        "No provider credential or memory plaintext in the report pack",
        evidence_findings.is_empty(),
        json!({
            "credentialMarkersScanned": policy.secrets.len(),
            "plaintextStringsScanned": policy.plaintext.len(),
            "evidenceFindings": evidence_findings,
            "scannedScope": "every file written into the pack directory",
            "verdictFile": "HYGIENE_OK or HYGIENE_FAIL beside MANIFEST.json",
        }),
    ));
    let dimensions = dimensions;

    let all_passed = dimensions.iter().all(|dimension| dimension.passed);
    let report = json!({
        "schemaVersion": 1,
        "profile": "a3s.code.dm-prod1-host.v1",
        "gate": "DM-PROD1 (HARNESS-CONV7)",
        "codeRevision": config.revision,
        "generatedAtUnixMs": chrono::Utc::now().timestamp_millis(),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "bindingMode": "DurableMemorySession::active_recall + with_semantic_recall",
        "parameters": {
            "activeSeedNodes": ACTIVE_SEED_NODES,
            "candidateSeedNodes": CANDIDATE_SEED_NODES,
            "peerAgentNodes": PEER_AGENT_NODES,
            "restartCycles": RESTART_CYCLES,
            "concurrentWriters": config.concurrent_writers,
            "refreshIntervalMs": REFRESH_INTERVAL.as_millis(),
            "candidateLimit": session::CANDIDATE_LIMIT,
            "embeddingBatchInputs": session::EMBEDDING_BATCH_INPUTS,
            "embeddingDimension": dimension,
        },
        "corpusDigests": {
            "targetNodeId": seeded.target_id,
            "targetContentDigest": seeded.target_content_digest,
            "primaryContentBytes": seeded.content_bytes,
            "peerContentBytes": peer_seeded.content_bytes,
            "seedChangeSets": seeded.change_sets + peer_seeded.change_sets,
        },
        "dimensions": dimensions,
        "passed": all_passed,
    });

    let pack = report::write_pack(config.pack_directory.clone(), &report, &policy).await?;

    // ---- Cleanup ------------------------------------------------------------
    // Scoped deletes only: the harness must leave the scratch database as it
    // found it without ever issuing FLUSHDB.
    index.destroy_keyspace().await?;
    lease_policy.destroy().await?;

    println!("dm-prod1 pack: {}", pack.directory.display());
    println!("dm-prod1 report: {}", pack.report_path.display());
    println!("dm-prod1 manifest: {}", pack.manifest_path.display());
    println!("dm-prod1 hygiene: {}", pack.hygiene_path.display());
    for dimension in &dimensions {
        println!(
            "  [{}] {} — {}",
            if dimension.passed { "PASS" } else { "FAIL" },
            dimension.id,
            dimension.dimension
        );
    }
    println!(
        "dm-prod1 provider: {} requests, {} tokens, ~${:.6} estimated",
        telemetry.requests, telemetry.total_tokens, estimated_usd
    );

    if !pack.hygiene_ok {
        anyhow::bail!(
            "secret hygiene failed: {}",
            pack.hygiene_findings.join("; ")
        );
    }
    if !all_passed {
        anyhow::bail!("DM-PROD1 host qualification did not satisfy every dimension");
    }
    Ok(())
}

fn refresh_elapsed_distribution(samples: &[f64]) -> Distribution {
    Distribution::from_samples(samples)
}

/// Valid unit vector used only to enumerate stored records through `search`.
fn enumeration_probe(dimension: usize) -> Vec<f32> {
    let mut values = vec![0.0f32; dimension];
    if let Some(first) = values.first_mut() {
        *first = 1.0;
    }
    values
}
