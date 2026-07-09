//! Real structured-output validation through the local Codex login.
//!
//! This is intentionally test-only. It reads `~/.codex/auth.json`, calls the
//! ChatGPT/Codex Responses SSE endpoint, adapts it to `LlmClient`, and then
//! exercises the normal a3s-code structured JSON and `generate_object` paths.
//!
//! Run from `crates/code`:
//!
//! ```bash
//! cargo test -p a3s-code-core --test test_structured_json_codex_login \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Set `A3S_CODEX_MODEL=<model>` to override the first listed local Codex model.

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::llm::structured::{
    generate_blocking, generate_streaming, PartialObjectCallback, StructuredMode,
    StructuredRequest, StructuredResult,
};
use a3s_code_core::llm::LlmClient;
use a3s_code_core::tools::{register_generate_object, ToolContext, ToolRegistry, ToolStreamEvent};
use serde_json::{json, Value};
use support::codex_login_client::{default_codex_model, CodexLoginClient};

const CALL_TIMEOUT: Duration = Duration::from_secs(180);

fn codex_client() -> Arc<dyn LlmClient> {
    let model = default_codex_model();
    eprintln!("[codex-login] model = {model}");
    Arc::new(
        CodexLoginClient::from_local_login(&model, "a3s-code-structured-json-test")
            .expect("local Codex login client"),
    )
}

fn person_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "age", "skills"],
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" },
            "skills": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 2
            },
            "city": { "type": "string" }
        }
    })
}

fn person_request(mode: StructuredMode) -> StructuredRequest {
    StructuredRequest {
        prompt: "Extract a structured person profile from this text: Alice is 30 years old, writes Rust and Python, and lives in Berlin.".to_string(),
        system: None,
        schema: person_schema(),
        schema_name: "person".to_string(),
        schema_description: Some("A person profile".to_string()),
        mode,
        max_repair_attempts: 2,
    }
}

fn assert_valid_person(object: &Value) {
    assert!(
        object["name"].is_string(),
        "name must be a string, got {object}"
    );
    assert!(
        object["age"].is_i64() || object["age"].is_u64(),
        "age must be an integer, got {object}"
    );
    assert!(
        object["skills"]
            .as_array()
            .is_some_and(|items| items.len() >= 2),
        "skills must be an array with at least two items, got {object}"
    );
}

async fn gen_with_timeout(
    client: &dyn LlmClient,
    request: &StructuredRequest,
) -> anyhow::Result<StructuredResult> {
    match tokio::time::timeout(CALL_TIMEOUT, generate_blocking(client, request)).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!("Codex structured call exceeded {CALL_TIMEOUT:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_structured_generation_is_stable() {
    let client = codex_client();

    for mode in [
        StructuredMode::Auto,
        StructuredMode::Tool,
        StructuredMode::Json,
        StructuredMode::Strict,
        StructuredMode::Prompt,
    ] {
        let result = gen_with_timeout(client.as_ref(), &person_request(mode))
            .await
            .unwrap_or_else(|error| {
                panic!("codex structured generation failed for {mode:?}: {error}")
            });
        assert_valid_person(&result.object);
        assert_eq!(
            result.mode_used,
            StructuredMode::Prompt,
            "test-only Codex login client should use prompt fallback"
        );
        assert!(
            result.usage.total_tokens > 0,
            "usage should include provider token totals"
        );
        eprintln!(
            "[codex-login:{mode:?}] repairs={} object={}",
            result.repair_rounds, result.object
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_structured_streaming_emits_valid_final_object() {
    let client = codex_client();
    let partials = Arc::new(std::sync::Mutex::new(0usize));
    let partials_cb = partials.clone();
    let on_partial: PartialObjectCallback = Box::new(move |_partial| {
        *partials_cb.lock().unwrap() += 1;
    });

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        generate_streaming(
            client.as_ref(),
            &person_request(StructuredMode::Auto),
            on_partial,
        ),
    )
    .await
    .expect("codex streaming structured call timed out")
    .expect("codex streaming structured generation failed");

    assert_valid_person(&result.object);
    assert!(
        result.usage.total_tokens > 0,
        "usage should include provider token totals"
    );
    eprintln!(
        "[codex-login:stream] partials={} object={}",
        *partials.lock().unwrap(),
        result.object
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_generate_object_tool_end_to_end() {
    let client = codex_client();
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let registry = Arc::new(ToolRegistry::new(workspace.path().to_path_buf()));
    register_generate_object(&registry, client);

    let object_args = |mode: &str| {
        json!({
            "schema_name": "person",
            "schema": person_schema(),
            "prompt": "Extract a person profile from: Alice is 30, writes Rust and Python, and lives in Berlin.",
            "mode": mode,
            "max_repair_attempts": 2
        })
    };

    for mode in ["auto", "tool", "json", "strict", "prompt"] {
        let output = tokio::time::timeout(
            CALL_TIMEOUT,
            registry.execute_raw_with_context(
                "generate_object",
                &object_args(mode),
                &ToolContext::new(workspace.path().to_path_buf()),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("codex generate_object mode {mode} timed out"))
        .unwrap_or_else(|error| panic!("codex generate_object mode {mode} errored: {error}"))
        .unwrap_or_else(|| panic!("generate_object tool missing from registry"));

        assert!(
            output.success,
            "codex generate_object mode {mode} failed: {output:?}"
        );
        let body: Value = serde_json::from_str(&output.content)
            .unwrap_or_else(|error| panic!("mode {mode}: output was not JSON: {error}"));
        assert_valid_person(&body["object"]);
        assert_eq!(body["mode_used"], "prompt");
        assert!(
            body["usage"]["total_tokens"].as_u64().unwrap_or_default() > 0,
            "mode {mode}: usage should include provider token totals: {body}"
        );
        assert_eq!(body.get("raw_text"), None);

        let metadata = output
            .metadata
            .as_ref()
            .unwrap_or_else(|| panic!("mode {mode}: metadata should be present"));
        assert_eq!(metadata["schema_name"], "person");
        assert_eq!(metadata["requested_mode"], mode);
        assert_eq!(metadata["raw_text_included"], false);
        eprintln!("[codex-login:generate_object:{mode}] {}", output.content);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_generate_object_handles_non_object_shapes_and_streaming() {
    let client = codex_client();
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let registry = Arc::new(ToolRegistry::new(workspace.path().to_path_buf()));
    register_generate_object(&registry, client);

    let shapes = [
        (
            "array",
            json!({
                "schema_name": "skills",
                "schema": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2
                },
                "prompt": "Return exactly two programming languages Alice knows from: Alice writes Rust and Python.",
                "mode": "auto",
                "max_repair_attempts": 2
            }),
        ),
        (
            "scalar",
            json!({
                "schema_name": "primary_language",
                "schema": {
                    "type": "string",
                    "enum": ["Rust", "Python", "TypeScript"]
                },
                "prompt": "Pick Alice's primary language from this text: Alice mainly writes Rust and also knows Python. Return one enum value.",
                "mode": "auto",
                "max_repair_attempts": 2,
                "include_raw_text": true
            }),
        ),
        (
            "array_object",
            json!({
                "schema_name": "work_items",
                "schema": {
                    "type": "array",
                    "minItems": 2,
                    "items": {
                        "type": "object",
                        "required": ["title", "priority"],
                        "additionalProperties": false,
                        "properties": {
                            "title": { "type": "string" },
                            "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                        }
                    }
                },
                "prompt": "Convert these work items into an array of objects: 1. Fix the auth bug, priority high. 2. Update the docs, priority low. Use lowercase enum values for priority.",
                "mode": "auto",
                "max_repair_attempts": 2
            }),
        ),
    ];

    for (shape, args) in shapes {
        let output = tokio::time::timeout(
            CALL_TIMEOUT,
            registry.execute_raw_with_context(
                "generate_object",
                &args,
                &ToolContext::new(workspace.path().to_path_buf()),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("codex generate_object {shape} timed out"))
        .unwrap_or_else(|error| panic!("codex generate_object {shape} errored: {error}"))
        .unwrap_or_else(|| panic!("generate_object tool missing from registry"));

        assert!(
            output.success,
            "codex generate_object {shape} failed: {output:?}"
        );
        let body: Value = serde_json::from_str(&output.content)
            .unwrap_or_else(|error| panic!("{shape}: output was not JSON: {error}"));
        match shape {
            "array" => assert!(
                body["object"]
                    .as_array()
                    .is_some_and(|items| items.len() >= 2),
                "array schema did not produce an array: {body}"
            ),
            "scalar" => assert!(
                body["object"].is_string(),
                "scalar schema did not produce a string: {body}"
            ),
            "array_object" => {
                let items = body["object"]
                    .as_array()
                    .unwrap_or_else(|| panic!("array_object did not produce an array: {body}"));
                assert!(
                    items.len() >= 2,
                    "array_object should contain at least two items: {body}"
                );
                for item in items {
                    assert!(item["title"].is_string(), "missing title: {item}");
                    assert!(
                        matches!(item["priority"].as_str(), Some("high" | "medium" | "low")),
                        "invalid priority: {item}"
                    );
                }
            }
            _ => unreachable!(),
        }
        eprintln!("[codex-login:shape:{shape}] {}", output.content);
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let output = tokio::time::timeout(
        CALL_TIMEOUT,
        registry.execute_raw_with_context(
            "generate_object",
            &json!({
                "schema_name": "person",
                "schema": person_schema(),
                "prompt": "Extract a person profile from: Alice is 30, writes Rust and Python, and lives in Berlin.",
                "mode": "auto",
                "max_repair_attempts": 2
            }),
            &ToolContext::new(workspace.path().to_path_buf()).with_event_tx(tx),
        ),
    )
    .await
    .expect("codex streaming generate_object timed out")
    .expect("codex streaming generate_object errored")
    .expect("generate_object tool missing from registry");

    assert!(
        output.success,
        "streaming generate_object failed: {output:?}"
    );
    let body: Value = serde_json::from_str(&output.content)
        .expect("streaming generate_object output should be JSON");
    assert_valid_person(&body["object"]);

    let mut saw_final_delta = false;
    while let Ok(event) = rx.try_recv() {
        let ToolStreamEvent::OutputDelta(delta) = event;
        let parsed: Value = serde_json::from_str(&delta)
            .unwrap_or_else(|error| panic!("stream delta was not JSON: {error}; {delta}"));
        if parsed["final"] == true {
            saw_final_delta = true;
            assert_valid_person(&parsed["object_partial"]);
            assert_eq!(parsed["mode_used"], "prompt");
        }
    }
    assert!(
        saw_final_delta,
        "streaming generate_object should emit a final delta"
    );
}
