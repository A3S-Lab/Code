#![cfg(test)]

use super::*;

fn fact() -> ResearchEvidenceFactV1 {
    ResearchEvidenceFactV1::new(
        "run-1",
        1,
        ResearchEvidenceFactKindV1::Source,
        "source-1",
        None,
        format!("sha256:{}", "a".repeat(64)),
        BTreeMap::new(),
        1,
    )
    .expect("fact")
}

#[test]
fn snapshot_binding_is_optional_canonical_and_digest_bound() {
    let original = fact();
    assert_eq!(original.source_snapshot_digest(), None);
    let snapshot = format!("sha256:{}", "b".repeat(64));
    let bound = original
        .clone()
        .with_source_snapshot_digest(&snapshot)
        .expect("snapshot");
    assert_eq!(bound.source_snapshot_digest(), Some(snapshot.as_str()));
    assert_ne!(original.fact_digest, bound.fact_digest);
    assert_eq!(
        ResearchEvidenceFactV1::from_slice(&bound.to_vec().expect("encode")).expect("decode"),
        bound
    );
    let mut tampered = bound.clone();
    tampered.metadata.insert(
        SOURCE_SNAPSHOT_METADATA_KEY.to_owned(),
        format!("sha256:{}", "c".repeat(64)),
    );
    assert!(matches!(
        tampered.validate(),
        Err(ResearchContractError::DigestMismatch("factDigest"))
    ));
}

#[test]
fn snapshot_binding_refuses_invalid_digest_and_corrupt_original() {
    assert!(fact().with_source_snapshot_digest("not-a-digest").is_err());
    let mut corrupt = fact();
    corrupt.subject_id = "changed".to_owned();
    assert!(corrupt
        .with_source_snapshot_digest(format!("sha256:{}", "b".repeat(64)))
        .is_err());
}
