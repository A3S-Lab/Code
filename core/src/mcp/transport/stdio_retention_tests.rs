#![cfg(test)]
#![cfg(unix)]

use std::future::Future;
use std::sync::atomic::AtomicUsize;
use std::task::Poll;

use super::*;
use crate::capability::{RetainedUseGeneration, Sha256Digest, UseCapabilityGeneration};

struct Retention {
    generation: UseCapabilityGeneration,
    released: Arc<AtomicUsize>,
}

impl RetainedUseGeneration for Retention {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
}
impl Drop for Retention {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_a_pending_close_preserves_process_retention_and_future_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let started = root.path().join("pid");
    let released = Arc::new(AtomicUsize::new(0));
    let retention = Arc::new(Retention {
        generation: UseCapabilityGeneration::selection(
            "test",
            1,
            Sha256Digest::new(format!("sha256:{}", "ab".repeat(32))).unwrap(),
        )
        .unwrap(),
        released: released.clone(),
    });
    let args = vec![
        "-c".into(),
        "printf '%s' \"$$\" > \"$1\"; exec sleep 30".into(),
        "fixture".into(),
        started.to_str().unwrap().into(),
    ];
    let transport = StdioTransport::spawn_with_timeout_and_retention(
        "/bin/sh",
        &args,
        &HashMap::new(),
        1,
        Some(retention),
        None,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut close = Box::pin(transport.close());
    futures::future::poll_fn(|cx| {
        assert!(matches!(close.as_mut().poll(cx), Poll::Pending));
        Poll::Ready(())
    })
    .await;
    drop(close);
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "dropping a close future released the process's retained resource"
    );
    transport.close().await.unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    let pid = std::fs::read_to_string(started).unwrap();
    let status = std::process::Command::new("kill")
        .args(["-0", &pid])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "close acknowledged while the actual process was still alive"
    );
    transport.close().await.unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
}
