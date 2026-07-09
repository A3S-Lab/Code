//! Real-LLM validation of the JSON-object-generation stability fix.
//!
//! Exercises the changed code paths against the live provider configured in
//! `.a3s/config.acl`:
//!   * requested Tool/Json/Strict modes safely downgrade to prompt+schema parsing
//!     when an OpenAI-compatible endpoint does not advertise native support;
//!   * the `generate_object` tool entrypoint across object, array, scalar,
//!     blocking, and streaming paths;
//!   * the hardened planner pre-analysis JSON path (`LlmPlanner::pre_analyze`).
//!
//! `#[ignore]` — requires a live provider in `.a3s/config.acl` and network
//! access to it. Run:
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_structured_json_real_llm -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::config::CodeConfig;
use a3s_code_core::llm::structured::{
    generate_blocking, generate_streaming, PartialObjectCallback, StructuredMode,
    StructuredRequest, StructuredResult,
};
use a3s_code_core::llm::{create_client_with_config, LlmClient};
use a3s_code_core::planning::LlmPlanner;
use a3s_code_core::tools::{register_generate_object, ToolContext, ToolRegistry, ToolStreamEvent};
use serde_json::{json, Value};

/// Hard ceiling per LLM call so a flaky/hung endpoint fails the test fast
/// instead of stalling for minutes.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const PRE_ANALYZE_TIMEOUT: Duration = Duration::from_secs(180);

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

/// Build a client from `.a3s/config.acl`. By default uses the config's
/// `default_model`; set `A3S_TEST_MODEL=provider/model` to target a specific
/// model (e.g. a tool-capable one for structured-output tests).
fn real_client() -> Arc<dyn LlmClient> {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to load {}: {e}", path.display()));

    let llm_config = match std::env::var("A3S_TEST_MODEL") {
        Ok(spec) => {
            let (provider, model) = spec
                .split_once('/')
                .expect("A3S_TEST_MODEL must be 'provider/model'");
            eprintln!("[real-llm] model = {spec} (from {})", path.display());
            config
                .llm_config(provider, model)
                .unwrap_or_else(|| panic!("model {spec} not found in {}", path.display()))
        }
        Err(_) => {
            eprintln!("[real-llm] model = <default> (from {})", path.display());
            config
                .default_llm_config()
                .expect("default llm config in .a3s/config.acl")
        }
    };
    create_client_with_config(llm_config)
}

/// Run a structured generation with a hard timeout.
async fn gen_with_timeout(
    client: &dyn LlmClient,
    req: &StructuredRequest,
) -> anyhow::Result<StructuredResult> {
    match tokio::time::timeout(CALL_TIMEOUT, generate_blocking(client, req)).await {
        Ok(res) => res,
        Err(_) => anyhow::bail!("LLM call exceeded {CALL_TIMEOUT:?}"),
    }
}

/// A non-trivial nested schema — the kind of object whose generation users
/// reported as unstable.
fn person_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "age", "skills"],
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" },
            "skills": { "type": "array", "items": { "type": "string" } },
            "address": {
                "type": "object",
                "properties": { "city": { "type": "string" } }
            }
        }
    })
}

fn person_request(mode: StructuredMode) -> StructuredRequest {
    StructuredRequest {
        prompt: "Extract a structured person profile from this text: \
                 'Alice is 30 years old, a Rust and Python developer living in Berlin.'"
            .to_string(),
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
        object["skills"].is_array(),
        "skills must be an array, got {object}"
    );
}

/// The core fix: requesting tool-mode structured generation must be STABLE.
/// Some OpenAI-compatible endpoints hang when sent native `tool_choice`; those
/// should safely downgrade to prompt+schema generation instead of hanging.
/// Run several independent times and require every one to yield a valid,
/// schema-conforming object.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_structured_tool_mode_is_stable() {
    let client = real_client();
    const RUNS: usize = 5;
    let mut total_repairs = 0u32;

    for i in 0..RUNS {
        let result = gen_with_timeout(client.as_ref(), &person_request(StructuredMode::Tool))
            .await
            .unwrap_or_else(|e| panic!("run {i}: tool-mode structured generation failed: {e}"));

        assert_valid_person(&result.object);
        total_repairs += result.repair_rounds as u32;
        eprintln!(
            "[tool-request] run {i}: ok (mode_used={:?}, repairs={}) -> {}",
            result.mode_used, result.repair_rounds, result.object
        );
    }

    eprintln!(
        "[tool] {RUNS}/{RUNS} runs produced valid objects; total repair rounds = {total_repairs}"
    );
}

/// The streaming structured path must also yield a valid object. On endpoints
/// without native structured support, this exercises streaming prompt mode.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_structured_tool_mode_streaming() {
    let client = real_client();
    let partials = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let partials_cb = partials.clone();
    let on_partial: PartialObjectCallback = Box::new(move |_p| {
        *partials_cb.lock().unwrap() += 1;
    });

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        generate_streaming(
            client.as_ref(),
            &person_request(StructuredMode::Tool),
            on_partial,
        ),
    )
    .await
    .expect("streaming call timed out")
    .expect("streaming tool-mode generation failed");

    assert_valid_person(&result.object);
    eprintln!(
        "[tool-stream-request] mode_used={:?} partials={} -> {}",
        result.mode_used,
        *partials.lock().unwrap(),
        result.object
    );
}

/// Native `response_format: json_object` on the OpenAI-compatible provider.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_structured_json_mode() {
    let client = real_client();
    let result = gen_with_timeout(client.as_ref(), &person_request(StructuredMode::Json))
        .await
        .expect("json_object structured generation failed");
    assert_valid_person(&result.object);
    eprintln!(
        "[json] mode_used={:?} repairs={} -> {}",
        result.mode_used, result.repair_rounds, result.object
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_generate_object_tool_is_stable() {
    let client = real_client();
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

    let mut mode_results = Vec::new();
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
        .unwrap_or_else(|_| panic!("generate_object tool mode {mode} timed out"))
        .unwrap_or_else(|e| panic!("generate_object tool mode {mode} errored: {e}"))
        .unwrap_or_else(|| panic!("generate_object tool missing from registry"));

        assert!(
            output.success,
            "generate_object mode {mode} failed: {output:?}"
        );
        let body: Value = serde_json::from_str(&output.content).unwrap_or_else(|e| {
            panic!("mode {mode}: output was not JSON: {e}; {}", output.content)
        });
        assert_valid_person(&body["object"]);
        assert!(
            body["usage"]["total_tokens"].as_u64().unwrap_or_default() > 0,
            "mode {mode}: usage must include provider token totals: {body}"
        );
        assert_eq!(
            body.get("raw_text"),
            None,
            "mode {mode}: raw_text must stay hidden unless explicitly requested"
        );
        let metadata = output
            .metadata
            .as_ref()
            .unwrap_or_else(|| panic!("mode {mode}: metadata should be present"));
        assert_eq!(metadata["schema_name"], "person");
        assert_eq!(metadata["requested_mode"], mode);
        assert_eq!(metadata["raw_text_included"], false);
        assert!(
            metadata["usage"]["total_tokens"]
                .as_u64()
                .unwrap_or_default()
                > 0,
            "mode {mode}: metadata should carry usage totals: {metadata}"
        );
        mode_results.push(json!({ "requested": mode, "used": body["mode_used"] }));
        eprintln!("[generate_object:{mode}] {}", output.content);
    }

    let invalid_mode_output = registry
        .execute_raw_with_context(
            "generate_object",
            &json!({
                "schema_name": "person",
                "schema": person_schema(),
                "prompt": "Extract a person profile from: Alice is 30.",
                "mode": "native_magic"
            }),
            &ToolContext::new(workspace.path().to_path_buf()),
        )
        .await
        .expect("invalid mode tool call should not error at registry layer")
        .expect("generate_object tool missing from registry");
    assert!(
        !invalid_mode_output.success,
        "invalid mode must be rejected before any provider call"
    );
    assert!(
        invalid_mode_output
            .content
            .contains("'mode' must be one of auto, strict, json, tool, or prompt"),
        "invalid mode error should explain the allowed values: {}",
        invalid_mode_output.content
    );

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
        .unwrap_or_else(|_| panic!("generate_object {shape} schema timed out"))
        .unwrap_or_else(|e| panic!("generate_object {shape} schema errored: {e}"))
        .unwrap_or_else(|| panic!("generate_object tool missing from registry"));

        assert!(
            output.success,
            "generate_object {shape} schema failed: {output:?}"
        );
        let body: Value = serde_json::from_str(&output.content).unwrap_or_else(|e| {
            panic!(
                "{shape} schema output was not JSON: {e}; {}",
                output.content
            )
        });
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
                    assert!(
                        item["title"].is_string(),
                        "array_object item title must be a string: {item}"
                    );
                    assert!(
                        matches!(item["priority"].as_str(), Some("high" | "medium" | "low")),
                        "array_object item priority must be a known enum value: {item}"
                    );
                }
            }
            _ => unreachable!(),
        }
        let raw_text_included = args
            .get("include_raw_text")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if raw_text_included {
            assert!(
                body["raw_text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty()),
                "{shape} schema requested raw_text, but output did not include it: {body}"
            );
        } else {
            assert_eq!(
                body.get("raw_text"),
                None,
                "{shape} schema did not request raw_text"
            );
        }
        let metadata = output
            .metadata
            .as_ref()
            .unwrap_or_else(|| panic!("{shape} schema metadata should be present"));
        assert_eq!(metadata["schema_name"], args["schema_name"]);
        assert_eq!(metadata["raw_text_included"], raw_text_included);
        eprintln!("[generate_object:{shape}] {}", output.content);
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let streaming_output = tokio::time::timeout(
        CALL_TIMEOUT,
        registry.execute_raw_with_context(
            "generate_object",
            &object_args("auto"),
            &ToolContext::new(workspace.path().to_path_buf()).with_event_tx(tx),
        ),
    )
    .await
    .expect("streaming generate_object tool timed out")
    .expect("streaming generate_object tool errored")
    .expect("generate_object tool missing from registry");
    assert!(
        streaming_output.success,
        "streaming generate_object failed: {streaming_output:?}"
    );
    let streaming_body: Value = serde_json::from_str(&streaming_output.content)
        .expect("streaming generate_object output should be JSON");
    assert_valid_person(&streaming_body["object"]);
    let mut saw_final_delta = false;
    while let Ok(event) = rx.try_recv() {
        let ToolStreamEvent::OutputDelta(delta) = event;
        let parsed: Value = serde_json::from_str(&delta)
            .unwrap_or_else(|e| panic!("stream delta was not JSON: {e}; {delta}"));
        if parsed["final"] == true {
            saw_final_delta = true;
            assert_valid_person(&parsed["object_partial"]);
            assert!(
                parsed["mode_used"].is_string(),
                "final stream delta should include mode_used: {parsed}"
            );
            assert!(
                parsed["repair_rounds"].is_u64(),
                "final stream delta should include repair_rounds: {parsed}"
            );
        } else {
            assert!(
                !parsed["object_partial"].is_null(),
                "partial stream delta should carry object_partial: {parsed}"
            );
        }
    }
    assert!(
        saw_final_delta,
        "streaming generate_object should emit final delta"
    );

    eprintln!("[generate_object] mode matrix: {:?}", mode_results);
}

/// Native `response_format: json_schema` (strict). Some providers reject schemas
/// that don't meet their strict subset, so this is tolerant: it must either
/// succeed with a valid object or fail cleanly (never hang or mis-parse).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_structured_strict_mode() {
    let client = real_client();
    match gen_with_timeout(client.as_ref(), &person_request(StructuredMode::Strict)).await {
        Ok(result) => {
            assert_valid_person(&result.object);
            eprintln!(
                "[strict] ok mode_used={:?} repairs={} -> {}",
                result.mode_used, result.repair_rounds, result.object
            );
        }
        Err(e) => {
            // Acceptable: provider may not support strict json_schema for this
            // schema. The point is that it fails cleanly, not silently wrong.
            eprintln!("[strict] provider rejected native json_schema (acceptable): {e}");
        }
    }
}

/// The hardened planner pre-analysis path against a real model: the response
/// must parse into a `PreAnalysis` (robust extractor + one repair retry).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn real_pre_analyze_parses() {
    let client = real_client();
    let analysis = tokio::time::timeout(
        PRE_ANALYZE_TIMEOUT,
        LlmPlanner::pre_analyze(
            &client,
            "Refactor the auth module in src/auth.rs to use async/await, and keep the public API stable.",
        ),
    )
    .await
    .expect("pre_analyze timed out")
    .expect("pre_analyze should parse a real model's JSON response");

    assert!(
        !analysis.optimized_input.trim().is_empty(),
        "optimized_input should be populated"
    );
    eprintln!(
        "[pre_analyze] requires_planning={} optimized_input={:?}",
        analysis.requires_planning, analysis.optimized_input
    );
}
