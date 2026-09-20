//! U-CV-03: session attachment admission is bounded and the accepted bytes
//! keep a stable digest. The read tool's output cap is a different path.

use super::*;
use crate::llm::{Attachment, MAX_ATTACHMENT_BYTES};
use sha2::{Digest, Sha256};

const PNG_BYTES: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const OVER_CAP_TOKEN: &[u8] = b"OVERCAP-BYTES-91";

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn reloaded_png_keeps_the_same_content_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.png");
    std::fs::write(&path, PNG_BYTES).unwrap();
    let first = Attachment::from_file(&path).unwrap();
    let second = Attachment::from_file(&path).unwrap();
    assert_eq!(first.media_type, "image/png");
    assert_eq!(sha256_hex(&first.data), sha256_hex(&second.data));
    assert_eq!(sha256_hex(&first.data), sha256_hex(PNG_BYTES));

    let ContentBlock::Image { source } = first.to_content_block() else {
        panic!("accepted png must become an image block");
    };
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(source.data.as_bytes())
        .unwrap();
    assert_eq!(sha256_hex(&decoded), sha256_hex(PNG_BYTES));
}

#[tokio::test]
async fn over_cap_attachment_is_rejected_before_history() {
    let config = crate::config::CodeConfig::from_acl(
        r#"
default_model = "anthropic/claude-sonnet-4-20250514"
providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" { name = "Claude Sonnet 4" }
}
"#,
    )
    .unwrap();
    let agent = crate::Agent::from_config(config).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    let mut data = vec![0_u8; MAX_ATTACHMENT_BYTES + 1];
    data[..OVER_CAP_TOKEN.len()].copy_from_slice(OVER_CAP_TOKEN);
    let error = session
        .send_with_attachments("describe", &[Attachment::png(data)], None)
        .await
        .expect_err("over-cap attachment must fail closed");
    let rendered = error.to_string();
    assert!(
        rendered.contains("attachment exceeds"),
        "typed size error missing: {rendered}"
    );
    assert!(
        !rendered.contains("OVERCAP-BYTES-91"),
        "size error included attachment bytes"
    );

    for message in session.history() {
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => {
                    assert!(!text.contains("OVERCAP-BYTES-91"));
                }
                ContentBlock::Image { source } => {
                    assert!(
                        source.data.len() < 64,
                        "over-cap image bytes entered history"
                    );
                    assert!(!source.data.contains("OVERCAP"));
                }
                _ => {}
            }
        }
    }
}
