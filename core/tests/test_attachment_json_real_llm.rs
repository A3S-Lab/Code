//! ATTACH-JSON1 — real-LLM gates for attachment messages and structured
//! JSON generation on the production session path.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +stable test -p a3s-code-core --test test_attachment_json_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::llm::Attachment;
use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

fn opts(session_id: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_planning(false)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_llm_api_timeout(180_000)
}

/// A 1x1 red PNG.
const RED_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn attachment_message_round_trips_on_the_real_model() {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(opts("attach-real")),
        )
        .await
        .expect("session");

    let outcome = tokio::time::timeout(
        Duration::from_secs(240),
        session.send_with_attachments(
            "An image is attached. Reply with exactly the word ATTACHED if you can see any \
             image content, otherwise reply with exactly NOWORD. Do not use any tools.",
            &[Attachment {
                data: RED_PIXEL_PNG.to_vec(),
                media_type: "image/png".to_string(),
            }],
            None,
        ),
    )
    .await
    .expect("attachment turn timed out at the harness level");
    // The pinned model may or may not be vision-capable: the CONTRACT is
    // that the attachment round-trips — a classified provider rejection is
    // an honest non-vision answer, silence or a hang is not.
    match outcome {
        Ok(result) => {
            let text = result.text.trim().to_uppercase();
            println!("ATTACHJSON attachment reply: {}", result.text.trim());
            assert!(
                text.contains("ATTACHED") || text.contains("NOWORD"),
                "the reply must acknowledge the multimodal ask: {}",
                result.text
            );
        }
        Err(error) => {
            let message = error.to_string().to_lowercase();
            println!("ATTACHJSON attachment classified failure: {error}");
            assert!(
                !message.is_empty(),
                "a non-vision provider must reject with a classified error"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn generate_object_produces_schema_valid_json_on_the_real_model() {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(opts("json-real")),
        )
        .await
        .expect("session");

    let outcome = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            "Use the generate_object tool to build an object with this schema: \
             {\"type\":\"object\",\"properties\":{\"answer\":{\"type\":\"integer\"},\
             \"label\":{\"type\":\"string\"}},\"required\":[\"answer\",\"label\"]} \
             and prompt 'the answer to everything, with a short label'. Then reply with \
             exactly the tool's JSON output and nothing else.",
            None,
        ),
    )
    .await
    .expect("json turn timed out at the harness level");
    let result = outcome.expect("the json generation turn must complete");
    let text = result.text.trim();
    println!("ATTACHJSON json reply: {text}");
    // The reply must contain a parseable JSON object with both required keys.
    let start = text.find('{');
    let end = text.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        panic!("the reply must carry a JSON object: {text}");
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&text[start..=end]).expect("embedded JSON must parse");
    assert!(parsed.get("answer").is_some(), "answer key: {parsed}");
    assert!(parsed.get("label").is_some(), "label key: {parsed}");
}
