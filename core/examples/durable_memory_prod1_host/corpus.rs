//! Deterministic operational corpus seeded into `FileMemoryRepository`.
//!
//! Content is only ever hashed into a report. Node text stays inside the
//! repository and the embedding request so a qualification pack cannot leak
//! prompt plaintext.

use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, MemoryChangeSet, MemoryNamespace,
    MemoryNodeDraft, MemoryOperation, MemoryRelation, MemoryRelationKind, MemoryRepository,
    MemoryStatus, RevisionMode, MAX_CHANGE_OPERATIONS,
};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};

/// Node index holding the uniquely recallable target statement.
pub const TARGET_INDEX: usize = 7;
/// Node index revised at the drift horizon.
pub const DRIFT_INDEX: usize = 3;

/// Paraphrase of the target statement; no corpus node repeats this wording.
pub const TARGET_QUERY: &str =
    "which runbook step covers restoring the payment ledger after rotating the amber gateway credential";

/// Distinct operational statements. Each is used at most once per corpus so a
/// real embedding provider produces an unambiguous nearest neighbour.
const TOPICS: &[&str] = &[
    "Drain the Helsinki edge pool before applying the kernel live-patch bundle.",
    "Reconcile the invoice dispute queue against the settlement export each Tuesday.",
    "Archive stale build caches once the artifact retention window closes.",
    "Validate the tenant isolation matrix after any subnet peering change.",
    "Escalate duplicate webhook deliveries to the integrations on-call engineer.",
    "Re-key the telemetry collector mTLS bundle ahead of the certificate expiry.",
    "Quarantine the flaky checkout regression suite before the release train departs.",
    "Rotate the amber gateway recovery credential before restoring the payment ledger service.",
    "Snapshot the analytics warehouse prior to the quarterly schema consolidation.",
    "Throttle the bulk notification fan-out when the mobile push backlog grows.",
    "Rebalance the search shard allocation after adding a replica to the cluster.",
    "Confirm the cold-storage restore drill completes inside the recovery objective.",
    "Freeze index compaction while the nightly reconciliation job holds its lock.",
    "Rehearse the regional evacuation runbook with the traffic director in dry-run mode.",
    "Audit the service account inventory for unused deploy keys every sprint.",
    "Pin the dependency lockfile before promoting a candidate image to staging.",
    "Replay the dead-letter stream after the schema registry contract is corrected.",
    "Warm the recommendation cache before the storefront promotion window opens.",
    "Detach the orphaned block volumes reported by the capacity reconciler.",
    "Verify the audit log shipper checkpoint after any broker leadership change.",
    "Suppress duplicate paging alerts while the maintenance window is declared.",
    "Refresh the geolocation dataset before the compliance boundary review.",
    "Disable the experimental ranker when the latency budget alarm trips twice.",
    "Recompute the entitlement projection after a plan migration completes.",
];

/// Digest-only description of one seeded corpus.
#[derive(Clone, Debug)]
pub struct SeededCorpus {
    pub active_ids: Vec<String>,
    pub candidate_ids: Vec<String>,
    pub content_bytes: usize,
    pub change_sets: usize,
    pub target_id: String,
    pub target_content_digest: String,
}

/// Create `active` Active nodes followed by `candidates` Candidate nodes.
pub async fn seed(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    active: usize,
    candidates: usize,
) -> Result<SeededCorpus> {
    anyhow::ensure!(
        active > TARGET_INDEX.max(DRIFT_INDEX),
        "the Active corpus must contain the target and drift nodes"
    );
    let total = active + candidates;
    let mut content_bytes = 0usize;
    let mut change_sets = 0usize;
    let mut target_content_digest = None;

    for start in (0..total).step_by(MAX_CHANGE_OPERATIONS) {
        let end = (start + MAX_CHANGE_OPERATIONS).min(total);
        let occurred_at = timestamp(i64::try_from(change_sets)?);
        let mut operations = Vec::with_capacity(end - start);
        for index in start..end {
            let content = content(index);
            content_bytes = content_bytes.saturating_add(content.len());
            if index == TARGET_INDEX {
                target_content_digest = Some(digest(&content));
            }
            let status = if index < active {
                MemoryStatus::Active
            } else {
                MemoryStatus::Candidate
            };
            operations.push(MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    node_id(namespace, index),
                    namespace.clone(),
                    DurableMemoryKind::Semantic,
                    status,
                    content,
                    vec![evidence(index, occurred_at)?],
                    occurred_at,
                )
                .with_confidence(0.9)
                .with_importance(0.7),
            });
        }
        repository
            .apply(MemoryChangeSet::new(
                format!("dm-prod1-seed-{}-{start:05}-{end:05}", namespace.scope_id()),
                namespace.clone(),
                occurred_at,
                operations,
            ))
            .await
            .with_context(|| format!("could not seed corpus range {start}..{end}"))?;
        change_sets += 1;
    }

    Ok(SeededCorpus {
        active_ids: (0..active).map(|index| node_id(namespace, index)).collect(),
        candidate_ids: (active..total)
            .map(|index| node_id(namespace, index))
            .collect(),
        content_bytes,
        change_sets,
        target_id: node_id(namespace, TARGET_INDEX),
        target_content_digest: target_content_digest
            .context("the target node was not constructed")?,
    })
}

/// Revise one Active node so the next refresh must re-embed exactly one input.
pub async fn revise(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    horizon: i64,
) -> Result<String> {
    let node_id = node_id(namespace, DRIFT_INDEX);
    let node = repository
        .get(namespace, &node_id)
        .await?
        .context("drift node is absent")?;
    let occurred_at = timestamp(10_000 + horizon);
    let content = format!(
        "{} Retention now closes after 21 days instead of 14.",
        content(DRIFT_INDEX)
    );
    repository
        .apply(MemoryChangeSet::new(
            format!("dm-prod1-drift-{horizon}"),
            namespace.clone(),
            occurred_at,
            vec![MemoryOperation::Revise {
                node_id: node_id.clone(),
                expected_revision: node.revision,
                content,
                mode: RevisionMode::Correction,
                evidence: vec![EvidenceRef::try_new(
                    format!("a3s://dm-prod1/drift/{horizon}"),
                    format!("sha256:{:064x}", horizon as u64 + 1),
                    EvidenceKind::Verification,
                    occurred_at,
                )?],
                confidence: Some(0.95),
                importance: Some(0.8),
            }],
        ))
        .await
        .context("could not persist the source drift")?;
    Ok(node_id)
}

/// Consolidate each `(superseded, survivor)` pair into the survivor.
///
/// The repository refuses a `Superseded` node that carries no `superseded_by`
/// relation, so consolidation is the three-operation shape: relate the loser to
/// the winner, relate the winner back, then demote the loser. All of it lands in
/// one change set so the graph invariant never observes a half-built pair.
pub async fn supersede(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    pairs: &[(String, String)],
    horizon: i64,
) -> Result<()> {
    let occurred_at = timestamp(20_000 + horizon);
    let mut operations = Vec::with_capacity(pairs.len() * 3);
    for (superseded_id, survivor_id) in pairs {
        anyhow::ensure!(
            superseded_id != survivor_id,
            "a memory node cannot supersede itself"
        );
        let superseded = repository
            .get(namespace, superseded_id)
            .await?
            .with_context(|| format!("consolidation target {superseded_id} is absent"))?;
        let survivor = repository
            .get(namespace, survivor_id)
            .await?
            .with_context(|| format!("consolidation survivor {survivor_id} is absent"))?;
        operations.push(MemoryOperation::AddRelation {
            node_id: superseded_id.clone(),
            expected_revision: superseded.revision,
            relation: MemoryRelation::new(MemoryRelationKind::SupersededBy, survivor_id.clone()),
        });
        operations.push(MemoryOperation::AddRelation {
            node_id: survivor_id.clone(),
            expected_revision: survivor.revision,
            relation: MemoryRelation::new(MemoryRelationKind::Supersedes, superseded_id.clone()),
        });
        // The relation bumped the loser one revision; demote that revision.
        operations.push(MemoryOperation::SetStatus {
            node_id: superseded_id.clone(),
            expected_revision: superseded.revision.saturating_add(1),
            status: MemoryStatus::Superseded,
        });
    }
    repository
        .apply(MemoryChangeSet::new(
            format!("dm-prod1-consolidate-{horizon}"),
            namespace.clone(),
            occurred_at,
            operations,
        ))
        .await
        .context("could not persist the consolidation horizon")?;
    Ok(())
}

/// Every Active node id currently projected by the namespace.
pub async fn active_node_ids(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    limit: usize,
) -> Result<Vec<String>> {
    let request = a3s_memory::repository::MemorySnapshotRequest::new(
        namespace.clone(),
        limit,
        a3s_memory::repository::MAX_SNAPSHOT_BYTES,
    )
    .with_statuses([MemoryStatus::Active]);
    let snapshot = repository.snapshot_namespace(request).await?;
    let mut ids: Vec<String> = snapshot
        .nodes()
        .iter()
        .map(|node| node.id.clone())
        .collect();
    ids.sort();
    Ok(ids)
}

/// Every literal string the hygiene scan must never find in a report pack.
pub fn plaintext_corpus() -> Vec<String> {
    let mut plaintext: Vec<String> = TOPICS.iter().map(|topic| topic.to_string()).collect();
    plaintext.push(TARGET_QUERY.to_string());
    plaintext
}

/// Evidence reference for an explicit activation decision.
pub fn activation_evidence(node_id: &str, horizon: i64) -> Result<EvidenceRef> {
    let occurred_at = timestamp(30_000 + horizon);
    EvidenceRef::try_new(
        format!("a3s://dm-prod1/activation/{node_id}"),
        format!("sha256:{:x}", Sha256::digest(node_id.as_bytes())),
        EvidenceKind::Verification,
        occurred_at,
    )
    .map_err(Into::into)
}

pub fn activation_timestamp(horizon: i64) -> DateTime<Utc> {
    timestamp(30_000 + horizon)
}

pub fn node_id(namespace: &MemoryNamespace, index: usize) -> String {
    // Scope must enter the ID so two namespaces that share one Redis index
    // cannot collide on the isolation check or the Active projection.
    format!("dm-prod1-{}-{index:05}", namespace.scope_id())
}

fn content(index: usize) -> String {
    let topic = TOPICS[index % TOPICS.len()];
    if index < TOPICS.len() {
        topic.to_string()
    } else {
        // Later cycles stay lexically distinct so duplicate embeddings cannot
        // make the target ambiguous.
        format!("Variant {index:05} of the operations handbook: {topic}")
    }
}

fn evidence(index: usize, occurred_at: DateTime<Utc>) -> Result<EvidenceRef> {
    EvidenceRef::try_new(
        format!("a3s://dm-prod1/evidence/{index:05}"),
        format!("sha256:{index:064x}"),
        EvidenceKind::Verification,
        occurred_at,
    )
    .map_err(Into::into)
}

fn timestamp(offset_seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + TimeDelta::seconds(offset_seconds)
}

fn digest(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}
