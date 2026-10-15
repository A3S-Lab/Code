#![cfg(test)]

use super::*;
use a3s_acl::{Document, Value};

fn block(name: &str, attributes: &[(&str, Value)]) -> Block {
    Block {
        name: name.into(),
        labels: Vec::new(),
        blocks: Vec::new(),
        attributes: attributes
            .iter()
            .map(|(key, value)| ((*key).into(), value.clone()))
            .collect(),
    }
}
fn text(value: &str) -> Value {
    Value::String(value.into())
}
fn number(value: u64) -> Value {
    Value::Number(value as f64)
}
fn list(values: &[&str]) -> Value {
    Value::List(values.iter().map(|value| text(value)).collect())
}
fn hash() -> Value {
    text(&format!("sha256:{}", "a".repeat(64)))
}

fn document() -> Document {
    let ids = (0..CASE_COUNT)
        .map(|index| format!("case {index} 甲 {{ literal }}\\tail"))
        .collect::<Vec<_>>();
    let mut oracle = block(
        "oracle",
        &[
            ("material_root", text("/private/acceptance/oracle")),
            ("verifier_digest", hash()),
            ("golden_digest", hash()),
            ("bug_digest", hash()),
            ("mutant_digest", hash()),
            ("case_count", number(16)),
        ],
    );
    oracle.blocks = ids
        .iter()
        .map(|id| {
            let mut case = block(
                "case",
                &[("input_digest", hash()), ("expected_output_digest", hash())],
            );
            case.labels.push(id.clone());
            case
        })
        .collect();
    Document {
        blocks: vec![
            block(
                "acceptance",
                &[
                    ("schema", text(MANIFEST_SCHEMA)),
                    ("manifest_version", number(1)),
                    ("task_id", text(" task 甲\\exact ")),
                    ("cluster_id", text("cluster 甲")),
                    ("variant", text("fixed")),
                    ("public_prompt_digest", hash()),
                    ("base_byte_digest", hash()),
                    (
                        "case_ids",
                        Value::List(ids.iter().map(|id| text(id)).collect()),
                    ),
                ],
            ),
            block(
                "candidate",
                &[
                    (
                        "allowed_paths",
                        list(&["entrypoint.py", " 金额 甲.py ", "literal\\name.py"]),
                    ),
                    ("entrypoint", text("entrypoint.py")),
                    ("max_files", number(512)),
                    ("max_path_bytes", number(4096)),
                    ("max_file_bytes", number(MAX_FILE_BYTES)),
                    ("max_total_bytes", number(MAX_TOTAL_BYTES)),
                ],
            ),
            oracle,
            block(
                "limits",
                &[
                    ("case_count", number(16)),
                    ("per_case_deadline_ms", number(2000)),
                    ("aggregate_deadline_ms", number(120000)),
                    ("request_bytes", number(16384)),
                    ("stdout_bytes", number(65536)),
                    ("stderr_bytes", number(65536)),
                    ("aggregate_output_bytes", number(2097152)),
                    ("host_artifact_bytes", number(MAX_TOTAL_BYTES)),
                    ("receipt_capture_bytes", number(MAX_FILE_BYTES)),
                    ("payload_bytes", number(262144)),
                    ("evidence_max_events", number(4096)),
                    ("evidence_max_event_bytes", number(16777216)),
                    ("evidence_max_artifacts", number(256)),
                    ("evidence_max_artifact_bytes", number(16777216)),
                    ("evidence_max_prompt_bytes", number(1048576)),
                    ("evidence_max_result_bytes", number(1048576)),
                ],
            ),
            block(
                "environment",
                &[
                    ("os", text("macos")),
                    ("arch", text("aarch64")),
                    ("sandbox_version", text("0.2.1")),
                    (
                        "sandbox_revision",
                        text("2b8687892bbb4a5e3d6e74fca42949a3f186d236"),
                    ),
                    ("backend", text("native-unqualified")),
                    ("environment_digest", hash()),
                    ("backend_digest", hash()),
                    ("policy_digest", hash()),
                    ("capability_digest", hash()),
                    ("qualification_digest", hash()),
                    ("interpreter", text("/usr/bin/python3")),
                    ("interpreter_digest", hash()),
                    ("interpreter_version", text("fixture identity")),
                    ("environment_allowlist", list(&["LANG"])),
                    ("maker_root", text("/private/acceptance/maker")),
                    ("candidate_root", text("/private/acceptance/candidate")),
                    ("scratch_root", text("/private/acceptance/scratch")),
                    ("evidence_root", text("/private/acceptance/evidence")),
                    ("result_root", text("/private/acceptance/results")),
                    ("other_attempt_roots", list(&["/private/acceptance/other"])),
                ],
            ),
        ],
    }
}
fn live() -> Block {
    block(
        "live",
        &[
            ("provider_model_route", text("declared/model")),
            ("provider_revision", Value::Null),
            ("model_revision", Value::Null),
            ("retry_policy_digest", hash()),
            ("fallback_policy_digest", hash()),
            ("admission_budget_digest", hash()),
            ("token_bound", number(100)),
            ("money_bound_microusd", text("100")),
            ("first_attempt_id", text("first")),
            ("remediation_attempt_id", text("remediation")),
            ("usage_uncertainty_policy", text("retain_unknown")),
        ],
    )
}
fn encode(document: &Document) -> String {
    a3s_acl::generate_acl(document)
}
fn parse(document: &Document) -> Result<AcceptanceManifestV1, ContractError> {
    AcceptanceManifestV1::parse_host(&encode(document))
}

#[test]
fn exact_spaces_unicode_backslashes_and_quoted_braces_survive_acl() {
    let parsed = parse(&document()).unwrap();
    assert_eq!(parsed.task_id, " task 甲\\exact ");
    assert_eq!(parsed.candidate.allowed_paths[1], " 金额 甲.py ");
    assert_eq!(parsed.candidate.allowed_paths[2], "literal\\name.py");
    assert_eq!(parsed.oracle.cases[0].case_id, parsed.case_ids[0]);
    assert!(parsed.live.is_none());
}

#[test]
fn unknown_duplicate_missing_and_workspace_authority_fields_refuse() {
    for unknown in ["providers", "sandbox", "permissions", "workspace"] {
        let mut data = document();
        data.blocks.push(block(unknown, &[]));
        assert!(parse(&data).is_err());
    }
    let mut data = document();
    data.blocks[0]
        .attributes
        .insert("unknown".into(), text("ignored?"));
    assert!(parse(&data).is_err());
    let mut data = document();
    let duplicate = data.blocks[0].clone();
    data.blocks.push(duplicate);
    assert!(parse(&data).is_err());
    let mut data = document();
    data.blocks[0].attributes.remove("base_byte_digest");
    assert!(parse(&data).is_err());
    let mut data = document();
    let duplicate = data.blocks[2].blocks[0].clone();
    data.blocks[2].blocks.push(duplicate);
    assert!(parse(&data).is_err());
    let mut data = document();
    data.blocks[0]
        .attributes
        .insert("case_ids".into(), list(&[]));
    assert!(parse(&data).is_err());
    let source = encode(&document()).replace(
        "manifest_version = 1",
        "manifest_version = 1\nmanifest_version = 1",
    );
    assert!(AcceptanceManifestV1::parse_host(&source)
        .unwrap_err()
        .to_string()
        .contains("duplicate attribute"));
    let source = encode(&document()).replacen(
        "input_digest =",
        "input_digest = \"ignored\"\ninput_digest =",
        1,
    );
    assert!(AcceptanceManifestV1::parse_host(&source).is_err());
}

#[test]
fn numeric_bounds_and_unfrozen_identities_refuse_without_rounding() {
    for (section, field, value) in [
        (1, "max_files", 513),
        (1, "max_path_bytes", 4097),
        (1, "max_file_bytes", MAX_FILE_BYTES + 1),
        (1, "max_total_bytes", MAX_TOTAL_BYTES + 1),
        (3, "case_count", 0),
        (3, "stdout_bytes", 65537),
        (3, "receipt_capture_bytes", MAX_FILE_BYTES + 1),
        (3, "evidence_max_events", 4097),
    ] {
        let mut data = document();
        data.blocks[section]
            .attributes
            .insert(field.into(), number(value));
        assert!(parse(&data).is_err(), "{field}");
    }
    for value in ["", "unpinned", "sha256:abc"] {
        let mut data = document();
        data.blocks[0]
            .attributes
            .insert("base_byte_digest".into(), text(value));
        assert!(parse(&data).is_err());
    }
    for raw in ["1.0", "1e0", "9007199254740993", "-1"] {
        let source = encode(&document())
            .replace("manifest_version = 1", &format!("manifest_version = {raw}"));
        assert!(AcceptanceManifestV1::parse_host(&source).is_err(), "{raw}");
    }
}

#[test]
fn live_requires_positive_admitted_bounds_and_exact_integer_string_money() {
    let mut data = document();
    data.blocks.push(live());
    let parsed = parse(&data).unwrap();
    assert_eq!(parsed.live.unwrap().money_bound_microusd, 100);
    for field in [
        "token_bound",
        "money_bound_microusd",
        "admission_budget_digest",
    ] {
        let mut missing = data.clone();
        missing.blocks[5].attributes.remove(field);
        assert!(matches!(
            parse(&missing),
            Err(ContractError::BudgetNotAdmitted(_))
        ));
    }
    for money in [
        Value::Number(1.0),
        text("1.0"),
        text("01"),
        text("0"),
        text("18446744073709551616"),
        Value::Null,
    ] {
        let mut invalid = data.clone();
        invalid.blocks[5]
            .attributes
            .insert("money_bound_microusd".into(), money);
        assert!(parse(&invalid).is_err());
    }
    let mut invalid = data.clone();
    invalid.blocks[5]
        .attributes
        .insert("token_bound".into(), number(0));
    assert!(matches!(
        parse(&invalid),
        Err(ContractError::BudgetNotAdmitted(_))
    ));
    let mut invalid = data;
    invalid.blocks[5]
        .attributes
        .insert("usage_uncertainty_policy".into(), text("missing_is_free"));
    assert!(parse(&invalid).is_err());
}

#[test]
fn paths_aliases_root_overlap_and_case_order_refuse() {
    for path in [
        "../entrypoint.py",
        "/entrypoint.py",
        "./entrypoint.py",
        "a//b.py",
        "a/../b.py",
        "a/",
    ] {
        assert!(
            validate_relative_path(path, MAX_PATH_BYTES).is_err(),
            "{path}"
        );
    }
    let mut data = document();
    data.blocks[2].blocks.swap(0, 1);
    assert!(parse(&data).is_err());
    let mut data = document();
    data.blocks[4].attributes.insert(
        "scratch_root".into(),
        text("/private/acceptance/candidate/scratch"),
    );
    assert!(parse(&data).is_err());
}

#[test]
fn manifest_identity_binds_exact_acl_bytes_and_has_a_bounded_input() {
    let source = encode(&document());
    let original = AcceptanceManifestV1::parse_host(&source).unwrap();
    let changed = AcceptanceManifestV1::parse_host(&format!("\n{source}")).unwrap();
    assert_ne!(original.manifest_digest, changed.manifest_digest);
    assert!(AcceptanceManifestV1::parse_host(&" ".repeat(MAX_MANIFEST_BYTES + 1)).is_err());
}

#[test]
#[cfg(unix)]
fn safe_file_load_refuses_links_fifo_directories_and_oversized_files_before_read() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("manifest.acl");
    std::fs::write(&path, encode(&document())).unwrap();
    assert!(AcceptanceManifestV1::load_host(&path).is_ok());
    let linked = root.join("linked.acl");
    symlink(&path, &linked).unwrap();
    assert!(AcceptanceManifestV1::load_host(&linked).is_err());
    let hard = root.join("hard.acl");
    std::fs::hard_link(&path, &hard).unwrap();
    assert!(AcceptanceManifestV1::load_host(&hard).is_err());
    std::fs::remove_file(hard).unwrap();
    let directory = root.join("directory.acl");
    std::fs::create_dir(&directory).unwrap();
    assert!(AcceptanceManifestV1::load_host(&directory).is_err());
    let fifo = root.join("fifo.acl");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // A controlled test FIFO has no writer; an attempted blocking open hangs.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(AcceptanceManifestV1::load_host(&fifo).is_err());
    let alias = root.join("alias");
    symlink(&root, &alias).unwrap();
    assert!(AcceptanceManifestV1::load_host(&alias.join("manifest.acl")).is_err());
    let file = std::fs::File::create(root.join("large.acl")).unwrap();
    file.set_len(MAX_MANIFEST_BYTES as u64 + 1).unwrap();
    assert!(AcceptanceManifestV1::load_host(&root.join("large.acl")).is_err());
}
