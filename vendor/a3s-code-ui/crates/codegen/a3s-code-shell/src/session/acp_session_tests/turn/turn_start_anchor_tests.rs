use super::disk_full_tests::{block_on_session, current_thread_local};
use super::support::*;
use super::*;
use std::sync::Arc;
use tokio::sync::mpsc;

#[test]
fn host_turn_stamps_fresh_turn_start() {
    block_on_session(|| {
        current_thread_local(async {
            let (gateway_tx, _gateway_rx) =
                mpsc::unbounded_channel::<a3s_acp_lib::AcpClientMessage>();
            let (persistence_tx, persistence_rx) = mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);
            let actor =
                Arc::new(create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await);

            // Stale anchor left by an inference turn 11 hours ago.
            let stale_ms = chrono::Utc::now().timestamp_millis() - 11 * 60 * 60 * 1000;
            actor.chat_state_handle.record_turn_start(stale_ms);

            let before_ms = chrono::Utc::now().timestamp_millis();
            Box::pin(actor.handle_prompt(
                "p-session-info",
                vec![acp::ContentBlock::Text(acp::TextContent::new(
                    "/session-info",
                ))],
                PromptMode::Agent,
                None,
                None,
                None,
                None,
                false,
                false,
                None,
                None,
                None,
            ))
            .await
            .expect("/session-info ends the turn host-side");

            let meta = actor
                .chat_state_handle
                .get_notification_meta()
                .await
                .expect("notification meta present after the turn");
            let anchor = meta.turn_start_ms.expect("turn_start_ms stamped");
            assert!(
                anchor >= before_ms,
                "kept stale anchor: {anchor} < {before_ms}"
            );
        });
    });
}
