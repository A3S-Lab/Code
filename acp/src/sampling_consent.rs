//! The per-request consent bridge for server-initiated MCP sampling
//! (SEC-SAMPLE1 precondition 2).
//!
//! A [`SamplingApprover`] that surfaces ONE human question per sampled
//! generation and waits for the host's answer: the agent loop serves
//! [`SamplingConsentAsk`] on the request_permission channel; this type
//! only sends and waits. Every non-approval — a denial, a dropped
//! channel, a host that never answers within the timeout — refuses the
//! sampled generation (fail closed).

use tokio::sync::{mpsc, oneshot};

/// How long a sampled generation waits for the host's consent before
/// refusing. The outer turn deadline still governs the whole call.
pub const CONSENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// One consent request parked for the agent loop: `question` is the
/// human-facing text (the server name and the excerpt), and `reply`
/// settles it — only `true` approves.
pub struct SamplingConsentAsk {
    pub server: String,
    pub question: String,
    pub reply: oneshot::Sender<bool>,
}

/// The consent half of the sampling wiring: `BoundedSamplingHandler`
/// wraps this OUTSIDE so even approved requests draw from the per-server
/// ceiling.
pub struct SamplingConsentBridge {
    asks: mpsc::Sender<SamplingConsentAsk>,
    timeout: std::time::Duration,
}

impl SamplingConsentBridge {
    pub fn new(asks: mpsc::Sender<SamplingConsentAsk>) -> Self {
        Self {
            asks,
            timeout: CONSENT_TIMEOUT,
        }
    }

    /// Timeout override for tests; the product wiring uses the default.
    #[cfg(test)]
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl a3s_code_core::mcp::SamplingApprover for SamplingConsentBridge {
    async fn approve_sampling(&self, server: &str, excerpt: &str) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        let question = format!(
            "The MCP server `{server}` requests one sampled model generation. Excerpt: {excerpt}"
        );
        let ask = SamplingConsentAsk {
            server: server.to_string(),
            question,
            reply: reply_tx,
        };
        // A closed consent surface is itself a refusal — never generate.
        if self.asks.send(ask).await.is_err() {
            return false;
        }
        match tokio::time::timeout(self.timeout, reply_rx).await {
            Ok(Ok(true)) => true,
            // Denial, dropped reply, or timeout: fail closed.
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::mcp::SamplingApprover as _;

    async fn bridged() -> (SamplingConsentBridge, mpsc::Receiver<SamplingConsentAsk>) {
        let (tx, rx) = mpsc::channel(1);
        (SamplingConsentBridge::new(tx), rx)
    }

    fn ask_params(server: &str) -> (&str, &str) {
        (server, "please summarize this file")
    }

    #[tokio::test]
    async fn an_answered_yes_approves_and_names_the_server() {
        let (bridge, mut rx) = bridged().await;
        let handle = tokio::spawn(async move {
            bridge
                .approve_sampling("probe", "please summarize this file")
                .await
        });
        let ask = rx.recv().await.expect("the ask is parked");
        assert!(
            ask.question.contains("probe"),
            "the human-facing question names the server: {}",
            ask.question
        );
        assert!(ask.question.contains("summarize"));
        ask.reply.send(true).expect("reply accepted");
        assert!(handle.await.expect("bridge settles"));
    }

    #[tokio::test]
    async fn a_denial_refuses() {
        let (bridge, mut rx) = bridged().await;
        let handle = tokio::spawn(async move { bridge.approve_sampling("probe", "excerpt").await });
        let ask = rx.recv().await.expect("the ask is parked");
        ask.reply.send(false).expect("reply accepted");
        assert!(!handle.await.expect("bridge settles"));
    }

    #[tokio::test]
    async fn a_dropped_reply_refuses() {
        let (bridge, mut rx) = bridged().await;
        let handle = tokio::spawn(async move { bridge.approve_sampling("probe", "excerpt").await });
        let ask = rx.recv().await.expect("the ask is parked");
        drop(ask); // the host side disappears without answering
        assert!(!handle.await.expect("bridge settles"));
    }

    #[tokio::test]
    async fn a_closed_consent_surface_refuses_without_parking() {
        let (bridge, rx) = bridged().await;
        drop(rx);
        let (server, excerpt) = ask_params("probe");
        assert!(
            !bridge.approve_sampling(server, excerpt).await,
            "no consent surface is itself a refusal — never generate"
        );
    }

    #[tokio::test]
    async fn an_unanswered_ask_times_out_refused() {
        let (tx, mut rx) = mpsc::channel(1);
        let bridge =
            SamplingConsentBridge::new(tx).with_timeout(std::time::Duration::from_millis(10));
        let handle = tokio::spawn(async move { bridge.approve_sampling("probe", "excerpt").await });
        let _ask = rx.recv().await.expect("the ask is parked");
        // No answer ever arrives; the short timeout refuses.
        assert!(!handle.await.expect("bridge settles"));
    }
}
