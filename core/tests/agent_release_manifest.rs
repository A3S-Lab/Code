use a3s_code_core::release::{
    agent_harness_compatibility_v1, AgentReleaseArtifact, AgentReleaseCacheMode,
    AgentReleaseCapability, AgentReleaseCompatibility, AgentReleaseEntrypoint, AgentReleaseError,
    AgentReleaseField, AgentReleaseHealth, AgentReleaseManifest, AgentReleasePersistentDataMode,
    AgentReleaseProvenance, AgentReleaseSecretRequirement, AgentReleaseSecretTarget,
    AgentReleaseStorage, AgentReleaseWorkspaceMode, AGENT_PROTOCOL_V1, AGENT_RELEASE_CONTRACT_V1,
    AGENT_RELEASE_ENTRYPOINT_ARGS_V1, AGENT_RELEASE_ENTRYPOINT_COMMAND_V1, AGENT_RELEASE_LIMITS,
};
use std::path::Path;

const RAW_FIXTURE: &str = include_str!("../../fixtures/agent-release-contract/.a3s/asset.acl");

fn fixture() -> &'static str {
    static NORMALIZED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NORMALIZED
        .get_or_init(|| RAW_FIXTURE.replace("\r\n", "\n").replace('\r', "\n"))
        .as_str()
}

fn compatibility() -> AgentReleaseCompatibility {
    AgentReleaseCompatibility::new(
        AGENT_PROTOCOL_V1,
        [
            AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            AgentReleaseCapability::new("secrets.external", 1).unwrap(),
            AgentReleaseCapability::new("workspace.local", 2).unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn native_harness_compatibility_is_code_owned_and_exact() {
    let compatibility = agent_harness_compatibility_v1();
    assert_eq!(compatibility.protocol(), AGENT_PROTOCOL_V1);
    assert_eq!(
        compatibility
            .capabilities()
            .iter()
            .map(|capability| (capability.name(), capability.level()))
            .collect::<Vec<_>>(),
        vec![
            ("runtime.service", 1),
            ("secrets.external", 1),
            ("workspace.local", 1),
        ]
    );
    AgentReleaseManifest::parse(fixture())
        .unwrap()
        .verify_compatibility(&compatibility)
        .unwrap();
}

#[test]
fn fixture_is_typed_canonical_and_digest_bound() {
    let manifest = AgentReleaseManifest::parse(fixture()).expect("fixture should be admitted");

    assert_eq!(manifest.contract(), AGENT_RELEASE_CONTRACT_V1);
    assert_eq!(manifest.protocol(), AGENT_PROTOCOL_V1);
    assert_eq!(
        manifest.artifact().digest(),
        "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    );
    assert_eq!(
        manifest.artifact().media_type(),
        "application/vnd.oci.image.manifest.v1+json"
    );
    assert_eq!(
        manifest.entrypoint().command(),
        AGENT_RELEASE_ENTRYPOINT_COMMAND_V1
    );
    assert_eq!(
        manifest.entrypoint().args(),
        AGENT_RELEASE_ENTRYPOINT_ARGS_V1
    );
    assert_eq!(manifest.health().transport(), "http");
    assert_eq!(manifest.health().port(), 8080);
    assert_eq!(manifest.health().readiness_path(), "/health/ready");
    assert_eq!(manifest.health().liveness_path(), "/health/live");
    assert_eq!(manifest.health().shutdown_grace_seconds(), 30);
    assert_eq!(
        manifest
            .required_capabilities()
            .iter()
            .map(|capability| (capability.name(), capability.level()))
            .collect::<Vec<_>>(),
        vec![
            ("runtime.service", 1),
            ("secrets.external", 1),
            ("workspace.local", 1)
        ]
    );
    assert_eq!(
        manifest.storage().workspace(),
        AgentReleaseWorkspaceMode::Ephemeral
    );
    assert_eq!(manifest.storage().cache(), AgentReleaseCacheMode::Ephemeral);
    assert_eq!(
        manifest.storage().persistent_data(),
        AgentReleasePersistentDataMode::None
    );
    assert_eq!(manifest.required_secrets().len(), 2);
    assert_eq!(manifest.required_secrets()[0].name(), "provider-api-key");
    assert_eq!(
        manifest.required_secrets()[0].target(),
        AgentReleaseSecretTarget::Environment
    );
    assert_eq!(
        manifest.required_secrets()[0].destination(),
        "PROVIDER_API_KEY"
    );
    assert_eq!(manifest.required_secrets()[1].name(), "signing-key");
    assert_eq!(
        manifest.required_secrets()[1].target(),
        AgentReleaseSecretTarget::File
    );
    assert_eq!(manifest.provenance().len(), 2);
    assert_eq!(manifest.provenance()[0].kind(), "builder");
    assert_eq!(
        manifest.provenance()[0].uri(),
        "urn:a3s:builder:github-actions"
    );
    assert_eq!(manifest.provenance()[1].kind(), "source");
    assert_eq!(
        manifest.provenance()[1].uri(),
        "https://github.com/A3S-Lab/Code"
    );
    assert!(manifest.identity().starts_with("sha256:"));
    assert_eq!(manifest.identity().len(), 71);
    assert_eq!(
        manifest.identity(),
        "sha256:d0f1bb153933320102b36703731096ea3030a949f9305a5f9837e7a4ba52e095"
    );
    assert!(manifest.canonical_acl().ends_with('\n'));

    let round_trip = AgentReleaseManifest::parse(manifest.canonical_acl())
        .expect("canonical bytes should parse");
    assert_eq!(round_trip.identity(), manifest.identity());
    assert_eq!(round_trip.canonical_acl(), manifest.canonical_acl());
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/agent-release-contract/.a3s/asset.acl");
    assert_eq!(
        AgentReleaseManifest::from_file(fixture_path)
            .expect("the conventional asset path should load")
            .identity(),
        manifest.identity()
    );
    manifest
        .verify_compatibility(&compatibility())
        .expect("fixture requirements should be available");

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentReleaseManifest>();
    assert_send_sync::<AgentReleaseError>();
}

#[test]
fn publication_binding_changes_only_artifact_and_exact_provenance() {
    let template = AgentReleaseManifest::parse(fixture()).expect("admitted publication template");
    let published = template
        .bind_publication(
            format!("sha256:{}", "a".repeat(64)),
            [
                AgentReleaseProvenance::new(
                    "source",
                    "urn:a3s:source:a3s-cli-binary",
                    format!("sha256:{}", "b".repeat(64)),
                )
                .unwrap(),
                AgentReleaseProvenance::new(
                    "builder",
                    "urn:a3s:builder:oci-buildkit-v1",
                    format!("sha256:{}", "c".repeat(64)),
                )
                .unwrap(),
            ],
        )
        .expect("bind final publication identity");

    assert_eq!(
        template.artifact().digest(),
        "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    );
    assert_eq!(
        published.artifact().digest(),
        format!("sha256:{}", "a".repeat(64))
    );
    assert_ne!(published.identity(), template.identity());
    assert_eq!(published.contract(), template.contract());
    assert_eq!(published.protocol(), template.protocol());
    assert_eq!(published.entrypoint(), template.entrypoint());
    assert_eq!(published.health(), template.health());
    assert_eq!(published.storage(), template.storage());
    assert_eq!(
        published.required_capabilities(),
        template.required_capabilities()
    );
    assert_eq!(published.required_secrets(), template.required_secrets());
    assert_eq!(published.provenance()[0].kind(), "builder");
    assert_eq!(
        published.provenance()[0].uri(),
        "urn:a3s:builder:oci-buildkit-v1"
    );
    assert_eq!(published.provenance()[1].kind(), "source");
    assert_eq!(
        published.provenance()[1].uri(),
        "urn:a3s:source:a3s-cli-binary"
    );
    assert!(!published.canonical_acl().contains(&"1".repeat(64)));
    assert!(!published.canonical_acl().contains(&"2".repeat(64)));
    assert!(!published.canonical_acl().contains(&"4".repeat(64)));
    assert_eq!(
        AgentReleaseManifest::parse(published.canonical_acl())
            .unwrap()
            .identity(),
        published.identity()
    );
}

#[test]
fn publication_binding_requires_the_exact_provenance_shape() {
    let template = AgentReleaseManifest::parse(fixture()).unwrap();
    let digest = format!("sha256:{}", "a".repeat(64));
    let source = AgentReleaseProvenance::new(
        "source",
        "urn:a3s:source:fixture",
        format!("sha256:{}", "b".repeat(64)),
    )
    .unwrap();

    let missing = template
        .bind_publication(&digest, [source.clone()])
        .expect_err("missing builder provenance must fail");
    assert_eq!(missing.field(), Some(AgentReleaseField::ProvenanceKind));

    let duplicate = template
        .bind_publication(&digest, [source.clone(), source])
        .expect_err("duplicate provenance must fail");
    assert_eq!(
        duplicate.code(),
        "a3s.code.agent_release.duplicate_provenance"
    );

    let attacker = "https://example.com/source?secret=TOP_SECRET_VALUE";
    let invalid =
        AgentReleaseProvenance::new("source", attacker, format!("sha256:{}", "b".repeat(64)))
            .expect_err("non-canonical provenance URI must fail");
    assert_eq!(invalid.field(), Some(AgentReleaseField::ProvenanceUri));
    assert!(!invalid.to_string().contains("TOP_SECRET_VALUE"));

    let invalid_artifact = template
        .bind_publication("sha256:TOP_SECRET_VALUE", [])
        .expect_err("invalid artifact digest must fail");
    assert_eq!(
        invalid_artifact.field(),
        Some(AgentReleaseField::ArtifactDigest)
    );
    assert!(!invalid_artifact.to_string().contains("TOP_SECRET_VALUE"));
}

#[test]
fn identity_ignores_formatting_and_set_like_block_order_only() {
    let first = AgentReleaseManifest::parse(fixture()).unwrap();
    let equivalent = fixture()
        .replace(
            "  capability \"runtime.service\" {\n    level = 1\n  }\n\n  capability \"secrets.external\" {\n    level = 1\n  }\n\n  capability \"workspace.local\" {\n    level = 1\n  }",
            "  # Capability requirements are a set.\n  capability \"workspace.local\" { level = 1 }\n  capability \"runtime.service\" { level = 1 }\n  capability \"secrets.external\" { level = 1 }",
        )
        .replace(
            "  schema = \"a3s.code.agent-release.v1\"\n  protocol = \"a3s.code.agent.v1\"",
            "  protocol = \"a3s.code.agent.v1\"\n  schema = \"a3s.code.agent-release.v1\"",
        )
        .replace(
            "  provenance \"source\" {\n    uri = \"https://github.com/A3S-Lab/Code\"\n    digest = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\"\n  }\n\n  provenance \"builder\" {\n    uri = \"urn:a3s:builder:github-actions\"\n    digest = \"sha256:4444444444444444444444444444444444444444444444444444444444444444\"\n  }",
            "  provenance \"builder\" { uri = \"urn:a3s:builder:github-actions\" digest = \"sha256:4444444444444444444444444444444444444444444444444444444444444444\" }\n  provenance \"source\" { uri = \"https://github.com/A3S-Lab/Code\" digest = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\" }",
        )
        .replace(
            "  secret \"provider-api-key\" {\n    target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"\n  }\n\n  secret \"signing-key\" {\n    target = \"file\"\n    destination = \"/run/secrets/signing-key\"\n  }",
            "  secret \"signing-key\" { target = \"file\" destination = \"/run/secrets/signing-key\" }\n  secret \"provider-api-key\" { target = \"environment\" destination = \"PROVIDER_API_KEY\" }",
        );
    let second = AgentReleaseManifest::parse(&equivalent).unwrap();

    assert_eq!(second.identity(), first.identity());
    assert_eq!(second.canonical_acl(), first.canonical_acl());

    let changed = fixture().replace(
        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "sha256:3333333333333333333333333333333333333333333333333333333333333333",
    );
    assert_ne!(
        AgentReleaseManifest::parse(&changed).unwrap().identity(),
        first.identity()
    );
}

#[test]
fn entrypoint_is_locked_to_the_a3s_code_harness() {
    let separate_harness = fixture().replace("/usr/bin/a3s", "/usr/bin/custom-agent-harness");
    let error = AgentReleaseManifest::parse(&separate_harness)
        .expect_err("a separate Harness executable must not be admitted");
    assert_eq!(error.field(), Some(AgentReleaseField::EntrypointCommand));

    let bypassed_code = fixture().replace(
        "args = [\"code\", \"harness\", \"--manifest\", \"/app/.a3s/asset.acl\"]",
        "args = [\"harness\", \"--manifest\", \"/app/.a3s/asset.acl\"]",
    );
    let error = AgentReleaseManifest::parse(&bypassed_code)
        .expect_err("the release must select the Code-owned Harness");
    assert_eq!(error.field(), Some(AgentReleaseField::EntrypointArgument));

    let reordered_args = fixture().replace(
        "args = [\"code\", \"harness\", \"--manifest\", \"/app/.a3s/asset.acl\"]",
        "args = [\"harness\", \"code\", \"--manifest\", \"/app/.a3s/asset.acl\"]",
    );
    let error = AgentReleaseManifest::parse(&reordered_args)
        .expect_err("entrypoint arguments must preserve the canonical command path");
    assert_eq!(error.field(), Some(AgentReleaseField::EntrypointArgument));
}

#[test]
fn admission_is_bounded_closed_and_value_redacting() {
    let unknown = fixture().replace(
        "  protocol = \"a3s.code.agent.v1\"",
        "  protocol = \"a3s.code.agent.v1\"\n  secret = \"TOP_SECRET\"",
    );
    let error = AgentReleaseManifest::parse(&unknown).expect_err("unknown field must fail");
    assert_eq!(error.code(), "a3s.code.agent_release.schema");
    assert!(!error.to_string().contains("TOP_SECRET"));

    let call = fixture().replace(
        "digest = \"sha256:1111111111111111111111111111111111111111111111111111111111111111\"",
        "digest = env(\"TOP_SECRET\")",
    );
    let error = AgentReleaseManifest::parse(&call).expect_err("function calls must fail");
    assert_eq!(error.code(), "a3s.code.agent_release.schema");
    assert!(!error.to_string().contains("TOP_SECRET"));

    let uppercase_digest = fixture().replace(
        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );
    let error =
        AgentReleaseManifest::parse(&uppercase_digest).expect_err("digest must be lowercase");
    assert_eq!(error.code(), "a3s.code.agent_release.invalid_field");
    assert!(!error.to_string().contains("AAAA"));

    let duplicate = fixture().replace(
        "  capability \"runtime.service\" {\n    level = 1\n  }",
        "  capability \"runtime.service\" {\n    level = 1\n  }\n  capability \"runtime.service\" { level = 2 }",
    );
    let error =
        AgentReleaseManifest::parse(&duplicate).expect_err("duplicate requirement must fail");
    assert_eq!(error.code(), "a3s.code.agent_release.duplicate_capability");

    let duplicate_provenance = fixture().replace("provenance \"source\"", "provenance \"builder\"");
    let error = AgentReleaseManifest::parse(&duplicate_provenance)
        .expect_err("provenance kinds must be unique");
    assert_eq!(error.code(), "a3s.code.agent_release.duplicate_provenance");

    let plaintext_secret = fixture().replace(
        "target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"",
        "target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"\n    value = \"TOP_SECRET\"",
    );
    let error = AgentReleaseManifest::parse(&plaintext_secret)
        .expect_err("secret plaintext fields must be closed");
    assert_eq!(error.code(), "a3s.code.agent_release.schema");
    assert!(!error.to_string().contains("TOP_SECRET"));

    let duplicate_secret = fixture().replace(
        "  secret \"provider-api-key\" {\n    target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"\n  }",
        "  secret \"provider-api-key\" {\n    target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"\n  }\n  secret \"provider-api-key\" { target = \"file\" destination = \"/run/secrets/other\" }",
    );
    let error =
        AgentReleaseManifest::parse(&duplicate_secret).expect_err("secret slots must be unique");
    assert_eq!(error.code(), "a3s.code.agent_release.duplicate_secret");

    let missing_secret_capability = fixture().replace(
        "  capability \"secrets.external\" {\n    level = 1\n  }\n\n",
        "",
    );
    let error = AgentReleaseManifest::parse(&missing_secret_capability)
        .expect_err("secret slots require the external-secret capability");
    assert_eq!(error.code(), "a3s.code.agent_release.invalid_field");

    let oversized = "x".repeat(AGENT_RELEASE_LIMITS.max_document_bytes + 1);
    let error = AgentReleaseManifest::parse(&oversized).expect_err("input must be bounded");
    assert_eq!(error.code(), "a3s.code.agent_release.parse");

    let attacker_named_field = fixture().replace(
        "  protocol = \"a3s.code.agent.v1\"",
        "  protocol = \"a3s.code.agent.v1\"\n  TOP_SECRET_VALUE = true",
    );
    let error = AgentReleaseManifest::parse(&attacker_named_field)
        .expect_err("unknown attacker-controlled field must fail");
    assert!(!error.to_string().contains("TOP_SECRET_VALUE"));
    assert!(!format!("{error:?}").contains("TOP_SECRET_VALUE"));
}

#[test]
fn compatibility_fails_before_activation_with_typed_reasons() {
    let manifest = AgentReleaseManifest::parse(fixture()).unwrap();

    let wrong_protocol = AgentReleaseCompatibility::new(
        "a3s.code.agent.v2",
        [
            AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            AgentReleaseCapability::new("secrets.external", 1).unwrap(),
            AgentReleaseCapability::new("workspace.local", 1).unwrap(),
        ],
    )
    .unwrap();
    let error = manifest
        .verify_compatibility(&wrong_protocol)
        .expect_err("protocol mismatch must fail");
    assert_eq!(error.code(), "a3s.code.agent_release.incompatible_protocol");

    let future_source = fixture().replace(AGENT_PROTOCOL_V1, "a3s.code.agent.v2");
    let future_manifest =
        AgentReleaseManifest::parse(&future_source).expect("schema and protocol are independent");
    let error = future_manifest
        .verify_compatibility(&compatibility())
        .expect_err("a valid but unavailable protocol must fail before activation");
    assert_eq!(error.code(), "a3s.code.agent_release.incompatible_protocol");

    let missing = AgentReleaseCompatibility::new(
        AGENT_PROTOCOL_V1,
        [
            AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            AgentReleaseCapability::new("secrets.external", 1).unwrap(),
        ],
    )
    .unwrap();
    let error = manifest
        .verify_compatibility(&missing)
        .expect_err("missing capability must fail");
    assert_eq!(
        error.code(),
        "a3s.code.agent_release.unsupported_capability"
    );
    assert!(matches!(
        error,
        AgentReleaseError::UnsupportedCapability { required_index: 2 }
    ));

    let invalid_level = AgentReleaseCapability::new("workspace.local", 0)
        .expect_err("zero capability level must fail");
    assert_eq!(invalid_level.code(), "a3s.code.agent_release.invalid_field");

    let demanding_source = fixture().replacen(
        "capability \"workspace.local\" {\n    level = 1",
        "capability \"workspace.local\" {\n    level = 2",
        1,
    );
    let demanding = AgentReleaseManifest::parse(&demanding_source).unwrap();
    let too_low = AgentReleaseCompatibility::new(
        AGENT_PROTOCOL_V1,
        [
            AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            AgentReleaseCapability::new("secrets.external", 1).unwrap(),
            AgentReleaseCapability::new("workspace.local", 1).unwrap(),
        ],
    )
    .unwrap();
    let error = demanding
        .verify_compatibility(&too_low)
        .expect_err("capability level mismatch must fail");
    assert_eq!(
        error.code(),
        "a3s.code.agent_release.unsupported_capability"
    );
}

#[test]
fn storage_and_secret_destinations_are_closed_and_collision_free() {
    for (source, replacement, field) in [
        (
            "workspace = \"ephemeral\"",
            "workspace = \"persistent\"",
            AgentReleaseField::WorkspaceMode,
        ),
        (
            "cache = \"ephemeral\"",
            "cache = \"external\"",
            AgentReleaseField::CacheMode,
        ),
        (
            "persistent_data = \"none\"",
            "persistent_data = \"ephemeral\"",
            AgentReleaseField::PersistentDataMode,
        ),
    ] {
        let invalid = fixture().replace(source, replacement);
        let error = AgentReleaseManifest::parse(&invalid)
            .expect_err("unknown storage mode must fail admission");
        assert_eq!(error.field(), Some(field));
    }

    for invalid_destination in [
        "provider_api_key",
        "/run/secrets/../signing-key",
        "/run/secrets//signing-key",
        "/run/secrets/signing-key/",
        "/tmp/signing-key",
    ] {
        let (source, replacement) = if invalid_destination == "provider_api_key" {
            (
                "destination = \"PROVIDER_API_KEY\"",
                format!("destination = \"{invalid_destination}\""),
            )
        } else {
            (
                "destination = \"/run/secrets/signing-key\"",
                format!("destination = \"{invalid_destination}\""),
            )
        };
        let invalid = fixture().replace(source, &replacement);
        let error = AgentReleaseManifest::parse(&invalid)
            .expect_err("non-canonical secret destination must fail admission");
        assert_eq!(error.field(), Some(AgentReleaseField::SecretDestination));
        assert!(!error.to_string().contains(invalid_destination));
    }

    let nested_file = fixture().replace(
        "destination = \"/run/secrets/signing-key\"",
        "destination = \"/run/secrets/signing/key.pem\"",
    );
    AgentReleaseManifest::parse(&nested_file)
        .expect("a canonical nested /run/secrets path should be admitted");

    let duplicate_destination = fixture().replace(
        "target = \"file\"\n    destination = \"/run/secrets/signing-key\"",
        "target = \"environment\"\n    destination = \"PROVIDER_API_KEY\"",
    );
    let error = AgentReleaseManifest::parse(&duplicate_destination)
        .expect_err("two slots must not inject into the same target");
    assert_eq!(error.code(), "a3s.code.agent_release.duplicate_secret");
}

#[test]
fn compatibility_inputs_are_canonical_unique_and_value_redacting() {
    let duplicate = AgentReleaseCompatibility::new(
        AGENT_PROTOCOL_V1,
        [
            AgentReleaseCapability::new("runtime.service", 1).unwrap(),
            AgentReleaseCapability::new("runtime.service", 2).unwrap(),
        ],
    )
    .expect_err("available capabilities must be unique");
    assert_eq!(
        duplicate.code(),
        "a3s.code.agent_release.duplicate_capability"
    );

    for protocol in ["a3s.code.agent.v0", "a3s.code.agent.v01", "agent.v1"] {
        let error = AgentReleaseCompatibility::new(protocol, [])
            .expect_err("non-canonical protocol versions must fail");
        assert_eq!(error.field(), Some(AgentReleaseField::Protocol));
    }

    let secret_looking_name = "top-secret-token";
    let source = fixture().replace("workspace.local", secret_looking_name);
    let manifest = AgentReleaseManifest::parse(&source).unwrap();
    let error = manifest
        .verify_compatibility(&compatibility())
        .expect_err("missing requirement must fail before activation");
    assert!(!error.to_string().contains(secret_looking_name));
    assert!(!format!("{error:?}").contains(secret_looking_name));
}

#[test]
fn file_admission_applies_encoding_and_size_budgets_before_parsing() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        vec![b'x'; AGENT_RELEASE_LIMITS.max_document_bytes + 1],
    )
    .unwrap();
    let error = AgentReleaseManifest::from_file(file.path()).expect_err("oversized file must fail");
    assert!(matches!(error, AgentReleaseError::InputTooLarge));

    std::fs::write(file.path(), [0xff]).unwrap();
    let error = AgentReleaseManifest::from_file(file.path()).expect_err("invalid UTF-8 must fail");
    assert!(matches!(error, AgentReleaseError::InvalidEncoding));
}

#[test]
fn public_release_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<AgentReleaseArtifact>();
    assert_send_sync::<AgentReleaseCacheMode>();
    assert_send_sync::<AgentReleaseCapability>();
    assert_send_sync::<AgentReleaseCompatibility>();
    assert_send_sync::<AgentReleaseEntrypoint>();
    assert_send_sync::<AgentReleaseError>();
    assert_send_sync::<AgentReleaseField>();
    assert_send_sync::<AgentReleaseHealth>();
    assert_send_sync::<AgentReleaseManifest>();
    assert_send_sync::<AgentReleasePersistentDataMode>();
    assert_send_sync::<AgentReleaseProvenance>();
    assert_send_sync::<AgentReleaseSecretRequirement>();
    assert_send_sync::<AgentReleaseSecretTarget>();
    assert_send_sync::<AgentReleaseStorage>();
    assert_send_sync::<AgentReleaseWorkspaceMode>();
}
