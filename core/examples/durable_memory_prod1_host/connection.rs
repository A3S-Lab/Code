//! Reconnecting single-socket Redis lease shared by the harness backends.
//!
//! `WATCH` is connection state. Multiplexing it would let an unrelated caller
//! interleave commands between the watch and `EXEC`, so every compare-and-swap
//! sequence takes exclusive ownership of one socket for its whole duration.
//! Failover is modelled by discarding a broken socket and reconnecting on the
//! next lease instead of propagating the I/O error to the caller.

use redis::aio::MultiplexedConnection;
use redis::{Client, RedisError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

/// Maximum times one logical operation re-establishes a dropped socket.
pub const MAX_RECONNECT_ATTEMPTS: usize = 4;

/// One exclusive Redis socket that transparently re-dials after a drop.
pub struct RedisSocket {
    client: Client,
    slot: Mutex<Option<MultiplexedConnection>>,
    reconnects: AtomicU64,
    dials: AtomicU64,
}

impl RedisSocket {
    /// Dial `url` once so configuration errors surface before any operation.
    pub async fn connect(url: &str) -> Result<Arc<Self>, RedisError> {
        let client = Client::open(url)?;
        let connection = client.get_multiplexed_async_connection().await?;
        Ok(Arc::new(Self {
            client,
            slot: Mutex::new(Some(connection)),
            reconnects: AtomicU64::new(0),
            dials: AtomicU64::new(1),
        }))
    }

    /// Number of times a dropped socket was replaced.
    pub fn reconnects(&self) -> u64 {
        self.reconnects.load(Ordering::SeqCst)
    }

    /// Total sockets dialled, including the initial connect.
    pub fn dials(&self) -> u64 {
        self.dials.load(Ordering::SeqCst)
    }

    /// Take exclusive ownership of the socket, re-dialling when it is absent.
    pub async fn lease(&self) -> Result<RedisLease<'_>, RedisError> {
        let mut slot = self.slot.lock().await;
        if slot.is_none() {
            let connection = self.client.get_multiplexed_async_connection().await?;
            self.dials.fetch_add(1, Ordering::SeqCst);
            *slot = Some(connection);
        }
        Ok(RedisLease { socket: self, slot })
    }
}

/// Exclusive borrow of the shared socket.
pub struct RedisLease<'a> {
    socket: &'a RedisSocket,
    slot: MutexGuard<'a, Option<MultiplexedConnection>>,
}

impl RedisLease<'_> {
    pub fn connection(&mut self) -> &mut MultiplexedConnection {
        self.slot
            .as_mut()
            .expect("a lease always holds a live connection")
    }

    /// Drop the socket so the next lease dials a replacement.
    pub fn invalidate(&mut self) {
        if self.slot.take().is_some() {
            self.socket.reconnects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Whether `error` means the socket is gone rather than the command rejected.
pub fn is_connection_loss(error: &RedisError) -> bool {
    error.is_connection_dropped()
        || error.is_io_error()
        || error.is_connection_refusal()
        || error.is_unrecoverable_error()
}
