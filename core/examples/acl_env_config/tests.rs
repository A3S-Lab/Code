#![cfg(test)]

use super::*;

const FIXTURE: &str = r#"default_model = "fixture-x/model"
// Nested blocks and braces in strings must not truncate the provider.
providers "fixture-x" { apiKey = "key-{literal}" baseUrl = "https://fixture.invalid/v1"
    models "model" { name = "Fixture" }
}
memory { llm_extraction = false }
"#;

#[test]
fn nested_acl_round_trip_preserves_other_blocks_and_env_calls() {
    let prepared = prepare(FIXTURE, |_| None).expect("fixture");
    assert_eq!(prepared.api_key.as_deref(), Some("key-{literal}"));
    let rendered = a3s_acl::generate(&prepared.document);
    assert!(!rendered.contains("key-{literal}"));
    let parsed = a3s_acl::parse(&rendered).expect("generated ACL");
    assert!(parsed.blocks.iter().any(|block| block.name == "memory"));
    let provider = parsed
        .blocks
        .iter()
        .find(|block| block.name == "providers")
        .expect("provider");
    assert_eq!(
        provider.attributes.get("api_key"),
        Some(&env_attribute("A3S_OPENAI_API_KEY"))
    );
    assert_eq!(
        provider.blocks[0].attributes.get("base_url"),
        Some(&env_attribute("A3S_OPENAI_BASE_URL"))
    );
}

#[test]
fn model_overrides_and_explicit_provider_env_have_correct_precedence() {
    let text = FIXTURE.replace(
        "name = \"Fixture\"",
        "name = \"Fixture\" api_key = \"model-key\"",
    );
    let model = prepare(&text, |_| None).expect("model override");
    assert_eq!(model.api_key.as_deref(), Some("model-key"));
    let host = prepare(&text, |name| {
        (name == "A3S_FIXTURE_X_API_KEY").then(|| "host-key".to_owned())
    })
    .expect("host override");
    assert_eq!(host.api_key.as_deref(), Some("host-key"));
}

#[test]
fn missing_credentials_remain_missing() {
    let text = r#"default_model = "fixture-x/model" providers "fixture-x" { models "model" {} }"#;
    let prepared = prepare(text, |_| None).expect("unconfigured fixture");
    assert!(prepared.api_key.is_none());
    assert!(prepared.base_url.is_none());
}

#[test]
fn duplicate_default_provider_is_refused() {
    let duplicate = format!("{FIXTURE}\nproviders \"fixture-x\" {{ models \"model\" {{}} }}");
    assert!(prepare(&duplicate, |_| None).is_err());
}

#[test]
fn invalid_acl_error_does_not_echo_credentials() {
    let error = prepare("providers { api_key = \"secret-token", |_| None)
        .err()
        .expect("invalid");
    assert_eq!(error.to_string(), "invalid ACL configuration");
    assert!(Mode::parse("unknown").is_err());
    assert_eq!(Mode::parse("check").expect("mode").as_str(), "check");
}

#[test]
fn input_is_bounded_and_requires_acl_extension() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("input.acl");
    fs::write(&path, FIXTURE).expect("fixture");
    assert_eq!(read_config(&path).expect("read"), FIXTURE);
    fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).expect("oversized");
    assert!(read_config(&path).is_err());
    assert!(read_config(dir.path()).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_is_refused_before_open() {
    let dir = tempfile::tempdir().expect("dir");
    let real = dir.path().join("real.acl");
    let link = dir.path().join("link.acl");
    fs::write(&real, FIXTURE).expect("fixture");
    std::os::unix::fs::symlink(real, &link).expect("symlink");
    assert!(read_config(&link).is_err());
}
