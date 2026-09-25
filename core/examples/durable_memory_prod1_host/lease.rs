//! Distributed refresh lease backed by `SET key value NX EX`.
//!
//! Only the lease holder may publish a semantic generation for a namespace.
//! Each grant carries a monotonic fence token from `INCR`, so a stalled holder
//! whose TTL expired can be detected and refused rather than silently
//! overwriting the newer holder's work. Release is guarded by an owner
//! comparison so an expired holder cannot delete a successor's lease.

use super::connection::{is_connection_loss, RedisSocket, MAX_RECONNECT_ATTEMPTS};
use redis::RedisError;
use std::sync::Arc;
use std::time::Duration;

/// Lua guard: delete only when the caller still owns the lease.
const RELEASE_SCRIPT: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

/// One namespace-scoped distributed lease.
pub struct RedisLeasePolicy {
    socket: Arc<RedisSocket>,
    lease_key: String,
    fence_key: String,
    ttl: Duration,
}

/// A granted lease, identified by its owner token and monotonic fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseGrant {
    pub owner: String,
    pub fence: u64,
}

/// Outcome of one acquisition attempt.
#[derive(Debug)]
pub enum LeaseAcquisition {
    Granted(LeaseGrant),
    Held { fence: Option<u64> },
}

impl RedisLeasePolicy {
    pub fn new(socket: Arc<RedisSocket>, prefix: &str, ttl: Duration) -> Self {
        Self {
            socket,
            lease_key: format!("{prefix}:lease"),
            fence_key: format!("{prefix}:lease-fence"),
            ttl,
        }
    }

    /// Try to take the lease for `owner`, minting a fresh fence token on grant.
    pub async fn acquire(&self, owner: &str) -> Result<LeaseAcquisition, RedisError> {
        let ttl_seconds = self.ttl.as_secs().max(1);
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            let fence_key = self.fence_key.clone();
            let owner = owner.to_string();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                let connection = lease.connection();
                let fence: u64 = redis::cmd("INCR")
                    .arg(&fence_key)
                    .query_async(connection)
                    .await?;
                let token = format!("{owner}#{fence}");
                let granted: Option<String> = redis::cmd("SET")
                    .arg(&lease_key)
                    .arg(&token)
                    .arg("NX")
                    .arg("EX")
                    .arg(ttl_seconds)
                    .query_async(connection)
                    .await?;
                if granted.is_some() {
                    return Ok(LeaseAcquisition::Granted(LeaseGrant {
                        owner: token,
                        fence,
                    }));
                }
                let current: Option<String> = redis::cmd("GET")
                    .arg(&lease_key)
                    .query_async(connection)
                    .await?;
                Ok(LeaseAcquisition::Held {
                    fence: current.as_deref().and_then(parse_fence),
                })
            })
        })
        .await
    }

    /// Whether `grant` is still the live holder for this namespace.
    pub async fn holds(&self, grant: &LeaseGrant) -> Result<bool, RedisError> {
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            let owner = grant.owner.clone();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                let current: Option<String> = redis::cmd("GET")
                    .arg(&lease_key)
                    .query_async(lease.connection())
                    .await?;
                Ok(current.as_deref() == Some(owner.as_str()))
            })
        })
        .await
    }

    /// Remaining TTL in seconds, or `None` when the lease has expired.
    pub async fn ttl_seconds(&self) -> Result<Option<i64>, RedisError> {
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                let ttl: i64 = redis::cmd("TTL")
                    .arg(&lease_key)
                    .query_async(lease.connection())
                    .await?;
                Ok((ttl >= 0).then_some(ttl))
            })
        })
        .await
    }

    /// Release the lease only when `grant` still owns it.
    pub async fn release(&self, grant: &LeaseGrant) -> Result<bool, RedisError> {
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            let owner = grant.owner.clone();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                let deleted: i64 = redis::Script::new(RELEASE_SCRIPT)
                    .key(&lease_key)
                    .arg(&owner)
                    .invoke_async(lease.connection())
                    .await?;
                Ok(deleted == 1)
            })
        })
        .await
    }

    /// Expire the lease immediately so a successor can be granted.
    ///
    /// The harness uses this to reach the post-TTL state without sleeping.
    pub async fn force_expire(&self) -> Result<(), RedisError> {
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                redis::cmd("DEL")
                    .arg(&lease_key)
                    .exec_async(lease.connection())
                    .await
            })
        })
        .await
    }

    /// Delete the lease and its fence counter, leaving no run residue behind.
    ///
    /// Only this policy's two keys are deleted; the database is never flushed.
    pub async fn destroy(&self) -> Result<(), RedisError> {
        self.retrying(|socket| {
            let lease_key = self.lease_key.clone();
            let fence_key = self.fence_key.clone();
            Box::pin(async move {
                let mut lease = socket.lease().await?;
                redis::cmd("DEL")
                    .arg(&lease_key)
                    .arg(&fence_key)
                    .exec_async(lease.connection())
                    .await
            })
        })
        .await
    }

    async fn retrying<T, F>(&self, mut operation: F) -> Result<T, RedisError>
    where
        F: FnMut(
            &Arc<RedisSocket>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, RedisError>> + Send + '_>,
        >,
    {
        let mut last: Option<RedisError> = None;
        for _ in 0..MAX_RECONNECT_ATTEMPTS {
            match operation(&self.socket).await {
                Ok(value) => return Ok(value),
                Err(error) if is_connection_loss(&error) => {
                    self.socket
                        .lease()
                        .await
                        .map(|mut lease| lease.invalidate())
                        .ok();
                    last = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last.unwrap_or_else(|| {
            RedisError::from((
                redis::ErrorKind::IoError,
                "lease operation exhausted retries",
            ))
        }))
    }
}

fn parse_fence(token: &str) -> Option<u64> {
    token.rsplit_once('#')?.1.parse().ok()
}
