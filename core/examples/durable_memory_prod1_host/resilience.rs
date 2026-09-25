//! Repeated restart and remote-backend failover evidence.

use super::connection::RedisSocket;
use super::redis_index::RedisVectorIndex;
use a3s_memory::vector::{VectorIndex, VectorIndexDescriptor};
use anyhow::{Context, Result};
use redis::RedisError;
use serde::Serialize;
use std::sync::Arc;

/// One reopen cycle: fresh sockets, fresh handles, same durable history.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartRecord {
    pub cycle: usize,
    pub history_digest_stable: bool,
    pub revision: u64,
    pub revision_preserved: bool,
    pub record_count: usize,
    pub records_preserved: bool,
    pub binding_digest_stable: bool,
    pub serving_generation_stable: bool,
    pub target_recall_rank: Option<usize>,
    pub reopen_ms: u64,
}

impl RestartRecord {
    pub fn passed(&self) -> bool {
        self.history_digest_stable
            && self.revision_preserved
            && self.records_preserved
            && self.binding_digest_stable
            && self.serving_generation_stable
            && self.target_recall_rank.is_some_and(|rank| rank <= 1)
    }
}

/// Failover outcome for the durable remote backend.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverEvidence {
    pub killed_clients: i64,
    pub reconnects: u64,
    pub sockets_dialled: u64,
    pub survived_connection_drop: bool,
    pub history_digest_stable_after_drop: bool,
    pub revision_preserved_after_drop: bool,
    pub records_preserved_after_drop: bool,
    pub recall_rank_after_drop: Option<usize>,
    pub alternate_history_rotated: bool,
    pub alternate_revision_reset: bool,
    pub primary_history_unaffected: bool,
    pub passed: bool,
}

/// Drop every other client connection to this Redis database.
///
/// `SKIPME yes` is the server default, so the issuing connection survives while
/// the index sockets are destroyed exactly as a failover would destroy them.
pub async fn kill_other_clients(socket: &RedisSocket) -> Result<i64, RedisError> {
    let mut lease = socket.lease().await?;
    redis::cmd("CLIENT")
        .arg("KILL")
        .arg("TYPE")
        .arg("normal")
        .arg("SKIPME")
        .arg("yes")
        .query_async(lease.connection())
        .await
}

/// Prove a recreated keyspace cannot masquerade as the surviving history.
///
/// Deletion is scoped to the alternate prefix; no database is flushed.
pub async fn rotate_alternate_keyspace(
    redis_url: &str,
    prefix: &str,
    descriptor: VectorIndexDescriptor,
) -> Result<(bool, bool)> {
    let socket = RedisSocket::connect(redis_url).await?;
    let alternate = RedisVectorIndex::open(Arc::clone(&socket), prefix, descriptor.clone()).await?;
    let before_history = alternate.history_digest().to_string();
    alternate
        .replace_partition(
            "alternate-partition",
            vec![a3s_memory::vector::VectorRecord::new(
                "alternate-record",
                unit_vector(descriptor.dimension),
            )],
        )
        .await?;
    let before_revision = alternate.observe().await?.status.revision.value();
    alternate.destroy_keyspace().await?;
    drop(alternate);

    let reopened = RedisVectorIndex::open(socket, prefix, descriptor).await?;
    let after_history = reopened.history_digest().to_string();
    let after_revision = reopened.observe().await?.status.revision.value();
    reopened.destroy_keyspace().await?;

    Ok((
        after_history != before_history,
        before_revision > 0 && after_revision == 0,
    ))
}

fn unit_vector(dimension: usize) -> Vec<f32> {
    let mut values = vec![0.0f32; dimension];
    if let Some(first) = values.first_mut() {
        *first = 1.0;
    }
    values
}

/// Digest of a session binding, used to prove exact resume identity.
pub fn binding_digest(binding: &a3s_code_core::DurableMemoryBindingV1) -> Result<String> {
    use sha2::{Digest, Sha256};
    let encoded = serde_json::to_vec(binding).context("binding is not serializable")?;
    Ok(format!("sha256:{:x}", Sha256::digest(&encoded)))
}
