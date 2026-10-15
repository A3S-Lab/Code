#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))]

use super::*;
use crate::behavior_acceptance::manifest::*;
use a3s_code_core::evaluation::EvidenceLimitsV1;
use std::cell::Cell;
use std::fs::{File, FileTimes, Permissions};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;

fn digest() -> DigestV1 {
    DigestV1::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}
fn binding() -> OriginalFreezeBindingV1 {
    OriginalFreezeBindingV1 {
        maker_target: ExecutionTargetV1::new("source-session", "source-run"),
        effect_digest: digest(),
        original_evidence_digest: digest(),
        original_receipt_capture_digest: digest(),
        settlement_evidence_digest: digest(),
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    manifest: AcceptanceManifestV1,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let maker = root.join("maker");
        std::fs::create_dir(&maker).unwrap();
        std::fs::write(maker.join("entry.py"), b"abc").unwrap();
        let manifest = AcceptanceManifestV1 {
            schema: MANIFEST_SCHEMA.into(),
            manifest_version: 1,
            manifest_digest: digest(),
            task_id: "freeze-unit".into(),
            cluster_id: "bytes".into(),
            variant: "unit".into(),
            public_prompt_digest: digest(),
            base_byte_digest: digest(),
            case_ids: (0..16).map(|n| format!("case-{n}")).collect(),
            candidate: CandidateDeclarationV1 {
                allowed_paths: vec!["entry.py".into()],
                entrypoint: "entry.py".into(),
                max_files: MAX_FILES,
                max_path_bytes: MAX_PATH_BYTES,
                max_file_bytes: MAX_FILE_BYTES,
                max_total_bytes: MAX_TOTAL_BYTES,
            },
            oracle: OracleDeclarationV1 {
                material_root: root.join("oracle"),
                verifier_digest: digest(),
                golden_digest: digest(),
                bug_digest: digest(),
                mutant_digest: digest(),
                case_count: 16,
                cases: (0..16)
                    .map(|n| OracleCaseV1 {
                        case_id: format!("case-{n}"),
                        input_digest: digest(),
                        expected_output_digest: digest(),
                    })
                    .collect(),
            },
            limits: AcceptanceLimitsV1 {
                case_count: 16,
                per_case_deadline_ms: 2000,
                aggregate_deadline_ms: 120000,
                request_bytes: 16384,
                stdout_bytes: 65536,
                stderr_bytes: 65536,
                aggregate_output_bytes: 2097152,
                host_artifact_bytes: MAX_TOTAL_BYTES,
                receipt_capture_bytes: MAX_FILE_BYTES,
                payload_bytes: 262144,
                evidence: EvidenceLimitsV1::default(),
            },
            environment: EnvironmentDeclarationV1 {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                sandbox_version: "0.2.1".into(),
                sandbox_revision: "2b8687892bbb4a5e3d6e74fca42949a3f186d236".into(),
                backend: "unqualified-unit-data".into(),
                environment_digest: digest(),
                backend_digest: digest(),
                policy_digest: digest(),
                capability_digest: digest(),
                qualification_digest: digest(),
                interpreter: PathBuf::from("/usr/bin/python3"),
                interpreter_digest: digest(),
                interpreter_version: "not-executed".into(),
                environment_allowlist: vec![],
                maker_root: maker,
                candidate_root: root.join("candidate"),
                scratch_root: root.join("scratch"),
                evidence_root: root.join("evidence"),
                result_root: root.join("results"),
                other_attempt_roots: vec![],
            },
            live: None,
        };
        Self { temp, manifest }
    }
    fn maker(&self) -> &Path {
        &self.manifest.environment.maker_root
    }
    fn candidate(&self) -> &Path {
        &self.manifest.environment.candidate_root
    }
    fn snapshot(&self, generation: u64) -> WorkspaceSourceSnapshotV1 {
        WorkspaceSourceSnapshotV1::new(
            self.maker().to_str().unwrap(),
            1,
            self.manifest.candidate.allowed_paths.len() as u64,
            digest().as_str(),
            1,
            1,
            1,
            generation,
            1,
        )
        .unwrap()
    }
    fn freeze(&self) -> Result<FrozenCandidateV1> {
        freeze_candidate(&self.manifest, binding(), || Ok(self.snapshot(1)))
    }
}
fn unseal(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        std::fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        for child in std::fs::read_dir(path).unwrap() {
            unseal(&child.unwrap().path());
        }
    } else if meta.is_file() {
        std::fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unseal(self.temp.path());
    }
}
fn rewrite_preserving_mtime(path: &Path, bytes: &[u8]) {
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
    std::fs::write(path, bytes).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o444)).unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn actual_bytes_are_sorted_exact_and_unused_files_are_absent() {
    let mut f = Fixture::new();
    std::fs::create_dir(f.maker().join("mód")).unwrap();
    for (name, bytes) in [
        ("mód/ 空白 .py", b"a\0b".as_slice()),
        ("literal\\name.py", b"backslash".as_slice()),
    ] {
        std::fs::write(f.maker().join(name), bytes).unwrap();
        f.manifest.candidate.allowed_paths.push(name.into());
    }
    std::fs::write(f.maker().join("not-allowed.py"), b"not copied").unwrap();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        Ok(f.snapshot(7))
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        (
            result.source_generation_before,
            result.source_generation_after
        ),
        (7, 7)
    );
    assert_eq!(
        result
            .files
            .iter()
            .map(|x| x.path.as_str())
            .collect::<Vec<_>>(),
        vec!["entry.py", "literal\\name.py", "mód/ 空白 .py"]
    );
    assert_eq!(result.total_bytes, 15);
    assert!(!f.candidate().join("not-allowed.py").exists());
    for entry in &result.files {
        assert_eq!(
            std::fs::read(f.maker().join(&entry.path)).unwrap(),
            std::fs::read(f.candidate().join(&entry.path)).unwrap()
        );
    }
    assert_eq!(result.original, binding());
    assert!(result.record_digest().is_ok());
    revalidate_frozen(&f.manifest, &result).unwrap();
}

#[test]
fn normative_length_framed_digest_matches_fixed_vector() {
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    assert_eq!(
        result.files[0].file_digest.as_str(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        result.candidate_byte_digest.as_str(),
        "sha256:bea9d3e98d4aba50b13a6dbe5c510895d54691adc16591971ea65a4922bb1c5d"
    );
    let mut changed = result.clone();
    changed.original.effect_digest = DigestV1::new(format!("sha256:{}", "b".repeat(64))).unwrap();
    assert_ne!(
        result.record_digest().unwrap(),
        changed.record_digest().unwrap()
    );
    assert_eq!(result.candidate_byte_digest, changed.candidate_byte_digest);
    let snapshot = serde_json::json!({
        "record": &result,
        "record_digest": result.record_digest().unwrap(),
        "actual_file_bytes": std::fs::read(f.candidate().join("entry.py")).unwrap(),
        "source_observation": "unit fixture using existing WorkspaceSourceSnapshotV1, not a live maker"
    }).to_string();
    assert!(snapshot.len() < 64 * 1024);
    eprintln!("frozen-candidate snapshot={snapshot}");
}

#[test]
fn source_same_length_preserved_mtime_rewrite_after_copy_refuses() {
    let f = Fixture::new();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            rewrite_preserving_mtime(&f.maker().join("entry.py"), b"xyz");
        }
        Ok(f.snapshot(1))
    });
    assert_eq!(calls.get(), 2);
    assert_eq!(result.unwrap_err().reason, AcceptanceReasonV1::FreezeRace);
}

#[test]
fn frozen_same_length_preserved_mtime_rewrite_cannot_reuse_record() {
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    rewrite_preserving_mtime(&f.candidate().join("entry.py"), b"xyz");
    assert_eq!(
        revalidate_frozen(&f.manifest, &result).unwrap_err().reason,
        AcceptanceReasonV1::StaleCandidate
    );
}

#[test]
fn changed_published_generation_refuses_even_with_unchanged_bytes() {
    let f = Fixture::new();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        Ok(f.snapshot(calls.get()))
    });
    assert_eq!(result.unwrap_err().reason, AcceptanceReasonV1::FreezeRace);
}

#[test]
fn foreign_snapshot_refuses_before_destination_creation() {
    let f = Fixture::new();
    let foreign =
        WorkspaceSourceSnapshotV1::new("/foreign", 1, 1, digest().as_str(), 1, 1, 1, 1, 1).unwrap();
    assert_eq!(
        freeze_candidate(&f.manifest, binding(), || Ok(foreign.clone()))
            .unwrap_err()
            .reason,
        AcceptanceReasonV1::FreezeInvalid
    );
    assert!(!f.candidate().exists());
}

#[test]
fn missing_source_is_not_successful_empty_capture() {
    let f = Fixture::new();
    std::fs::remove_file(f.maker().join("entry.py")).unwrap();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeIncomplete
    );
}

#[test]
fn symlink_to_external_writerless_fifo_is_refused_before_read() {
    let f = Fixture::new();
    let fifo = f.temp.path().join("external-fifo");
    make_fifo(&fifo);
    std::fs::remove_file(f.maker().join("entry.py")).unwrap();
    symlink(&fifo, f.maker().join("entry.py")).unwrap();
    let error = f.freeze().unwrap_err();
    assert_eq!(error.reason, AcceptanceReasonV1::FreezeInvalid);
    assert!(error.detail.contains("before open"));
}
fn make_fifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // Controlled no-writer FIFO; capture must refuse rather than block on open.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

#[test]
fn symlinked_source_ancestor_is_refused() {
    let mut f = Fixture::new();
    let outside = f.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("entry.py"), b"secret").unwrap();
    symlink(&outside, f.maker().join("alias")).unwrap();
    f.manifest.candidate.allowed_paths = vec!["alias/entry.py".into()];
    f.manifest.candidate.entrypoint = "alias/entry.py".into();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
    assert_eq!(std::fs::read(outside.join("entry.py")).unwrap(), b"secret");
}

#[test]
fn hardlinked_source_is_refused() {
    let f = Fixture::new();
    std::fs::hard_link(
        f.maker().join("entry.py"),
        f.temp.path().join("external-link"),
    )
    .unwrap();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
}

#[test]
fn fifo_source_is_refused_without_a_writer() {
    let f = Fixture::new();
    std::fs::remove_file(f.maker().join("entry.py")).unwrap();
    make_fifo(&f.maker().join("entry.py"));
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
}

#[test]
fn device_source_is_refused() {
    let mut f = Fixture::new();
    f.manifest.environment.maker_root = PathBuf::from("/dev");
    f.manifest.candidate.allowed_paths = vec!["null".into()];
    f.manifest.candidate.entrypoint = "null".into();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
}

#[test]
fn directory_source_is_refused() {
    let f = Fixture::new();
    std::fs::remove_file(f.maker().join("entry.py")).unwrap();
    std::fs::create_dir(f.maker().join("entry.py")).unwrap();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
}

#[test]
fn no_read_permission_source_is_refused_even_for_privileged_host() {
    let f = Fixture::new();
    std::fs::set_permissions(f.maker().join("entry.py"), Permissions::from_mode(0o0)).unwrap();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
}

#[test]
fn malformed_reserved_and_aliased_paths_refuse_before_observation() {
    for path in [
        "/entry.py",
        "../entry.py",
        "./entry.py",
        "x//entry.py",
        "x/../entry.py",
        ".git/config",
        ".a3s-code/log",
        ".a3s/config.acl",
        "docs/evidence/private",
        "nul\0.py",
    ] {
        let mut f = Fixture::new();
        f.manifest.candidate.allowed_paths = vec![path.into()];
        f.manifest.candidate.entrypoint = path.into();
        let calls = Cell::new(0);
        assert_eq!(
            freeze_candidate(&f.manifest, binding(), || {
                calls.set(calls.get() + 1);
                Ok(f.snapshot(1))
            })
            .unwrap_err()
            .reason,
            AcceptanceReasonV1::FreezeInvalid
        );
        assert_eq!(calls.get(), 0);
        assert!(!f.candidate().exists());
    }
    let mut f = Fixture::new();
    f.manifest.candidate.allowed_paths.push("entry.py".into());
    assert!(f.freeze().is_err());
}

#[test]
fn file_path_and_total_declaration_ceilings_cannot_be_widened() {
    let mut f = Fixture::new();
    f.manifest.candidate.allowed_paths = (0..513).map(|n| format!("{n}.py")).collect();
    f.manifest.candidate.entrypoint = "0.py".into();
    assert!(f.freeze().is_err());
    f.manifest.candidate.allowed_paths = vec!["x".repeat(4097)];
    f.manifest.candidate.entrypoint = "x".repeat(4097);
    assert!(f.freeze().is_err());
    for dimension in 0..4 {
        let mut f = Fixture::new();
        match dimension {
            0 => f.manifest.candidate.max_files = 513,
            1 => f.manifest.candidate.max_path_bytes = 4097,
            2 => f.manifest.candidate.max_file_bytes = MAX_FILE_BYTES + 1,
            _ => f.manifest.candidate.max_total_bytes = MAX_TOTAL_BYTES + 1,
        };
        assert!(f.freeze().is_err());
    }
}

#[test]
fn actual_oversized_file_refuses_before_copy() {
    let f = Fixture::new();
    File::options()
        .write(true)
        .open(f.maker().join("entry.py"))
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
    assert!(!f.candidate().join("entry.py").exists());
}

#[test]
fn narrowed_total_limit_is_enforced_on_actual_copied_bytes() {
    let mut f = Fixture::new();
    std::fs::write(f.maker().join("entry.py"), b"ab").unwrap();
    std::fs::write(f.maker().join("second.py"), b"cd").unwrap();
    f.manifest.candidate.allowed_paths.push("second.py".into());
    f.manifest.candidate.max_file_bytes = 2;
    f.manifest.candidate.max_total_bytes = 3;
    assert!(f.freeze().is_err());
    assert_eq!(
        std::fs::metadata(f.candidate().join("entry.py"))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        std::fs::metadata(f.candidate().join("second.py"))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn existing_candidate_root_is_never_merged_or_overwritten() {
    let f = Fixture::new();
    std::fs::create_dir(f.candidate()).unwrap();
    std::fs::write(f.candidate().join("sentinel"), b"original").unwrap();
    assert!(f.freeze().is_err());
    assert_eq!(
        std::fs::read(f.candidate().join("sentinel")).unwrap(),
        b"original"
    );
    assert!(!f.candidate().join("entry.py").exists());
}

#[test]
fn divergent_record_or_manifest_cannot_revalidate() {
    let mut f = Fixture::new();
    let result = f.freeze().unwrap();
    let mut changed = result.clone();
    changed.total_bytes += 1;
    assert!(revalidate_frozen(&f.manifest, &changed).is_err());
    changed = result.clone();
    changed.files[0].path = "other.py".into();
    assert!(changed.validate().is_err());
    f.manifest.manifest_digest = DigestV1::new(format!("sha256:{}", "b".repeat(64))).unwrap();
    assert!(revalidate_frozen(&f.manifest, &result).is_err());
}

#[test]
fn missing_frozen_file_refuses_current_reuse_without_changing_record() {
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    let original = serde_json::to_vec(&result).unwrap();
    unseal(f.candidate());
    std::fs::remove_file(f.candidate().join("entry.py")).unwrap();
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        revalidate_frozen(&f.manifest, &result).unwrap_err().reason,
        AcceptanceReasonV1::StaleCandidate
    );
    assert_eq!(serde_json::to_vec(&result).unwrap(), original);
}

#[test]
fn unexpected_frozen_module_refuses_current_reuse() {
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o700)).unwrap();
    std::fs::write(f.candidate().join("injected.py"), b"malicious").unwrap();
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        revalidate_frozen(&f.manifest, &result).unwrap_err().reason,
        AcceptanceReasonV1::StaleCandidate
    );
}

#[test]
fn source_directory_swap_to_external_symlink_cannot_redirect_rescan() {
    let mut f = Fixture::new();
    std::fs::create_dir(f.maker().join("nested")).unwrap();
    std::fs::write(f.maker().join("nested/entry.py"), b"abc").unwrap();
    f.manifest.candidate.allowed_paths = vec!["nested/entry.py".into()];
    f.manifest.candidate.entrypoint = "nested/entry.py".into();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            std::fs::rename(f.maker().join("nested"), f.temp.path().join("old")).unwrap();
            symlink(f.temp.path().join("old"), f.maker().join("nested")).unwrap();
        }
        Ok(f.snapshot(1))
    });
    assert_eq!(result.unwrap_err().reason, AcceptanceReasonV1::FreezeRace);
    assert_eq!(
        std::fs::read(f.temp.path().join("old/entry.py")).unwrap(),
        b"abc"
    );
}

#[test]
fn same_bytes_replacement_inode_is_a_source_race() {
    let f = Fixture::new();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            std::fs::rename(f.maker().join("entry.py"), f.maker().join("old.py")).unwrap();
            std::fs::write(f.maker().join("entry.py"), b"abc").unwrap();
        }
        Ok(f.snapshot(1))
    });
    assert_eq!(result.unwrap_err().reason, AcceptanceReasonV1::FreezeRace);
}

#[test]
fn missing_source_after_copy_is_a_race() {
    let f = Fixture::new();
    let calls = Cell::new(0);
    let result = freeze_candidate(&f.manifest, binding(), || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            std::fs::remove_file(f.maker().join("entry.py")).unwrap();
        }
        Ok(f.snapshot(1))
    });
    assert_eq!(result.unwrap_err().reason, AcceptanceReasonV1::FreezeRace);
}

#[test]
fn non_utf8_filename_is_refused_by_filesystem_or_tree_revalidation() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o700)).unwrap();
    let path = f.candidate().join(std::ffi::OsString::from_vec(vec![0xff]));
    let written = std::fs::write(&path, b"extra");
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o555)).unwrap();
    match written {
        Ok(()) => {
            assert_eq!(
                revalidate_frozen(&f.manifest, &result).unwrap_err().reason,
                AcceptanceReasonV1::StaleCandidate
            );
            eprintln!("non-UTF-8 qualification: actual frozen entry rejected by tree reader");
        }
        Err(error) => {
            assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
            assert!(!path.exists());
            revalidate_frozen(&f.manifest, &result).unwrap();
            eprintln!("non-UTF-8 qualification: filesystem rejected entry with EILSEQ; tree-decoder coverage is the separate byte-level unit test");
        }
    }
}

#[test]
fn replacement_frozen_root_with_identical_bytes_cannot_reuse_original_identity() {
    let f = Fixture::new();
    let result = f.freeze().unwrap();
    std::fs::rename(f.candidate(), f.temp.path().join("old-candidate")).unwrap();
    std::fs::create_dir(f.candidate()).unwrap();
    std::fs::write(f.candidate().join("entry.py"), b"abc").unwrap();
    std::fs::set_permissions(
        f.candidate().join("entry.py"),
        Permissions::from_mode(0o444),
    )
    .unwrap();
    std::fs::set_permissions(f.candidate(), Permissions::from_mode(0o555)).unwrap();
    assert_eq!(
        revalidate_frozen(&f.manifest, &result).unwrap_err().reason,
        AcceptanceReasonV1::StaleCandidate
    );
}

#[test]
fn unavailable_original_source_observation_is_not_replaced_with_a_scan() {
    let f = Fixture::new();
    let result = freeze_candidate(&f.manifest, binding(), || {
        Err(FreezeError::source_unavailable(
            "original snapshot unavailable",
        ))
    });
    let error = result.unwrap_err();
    assert_eq!(error.reason, AcceptanceReasonV1::EvidenceIncomplete);
    assert_eq!(error.detail, "original snapshot unavailable");
    assert!(!f.candidate().exists());
}

#[test]
fn exactly_512_actual_files_are_captured_and_revalidated() {
    let mut f = Fixture::new();
    for n in 0..511 {
        let name = format!("source-{n}.py");
        std::fs::write(f.maker().join(&name), b"x").unwrap();
        f.manifest.candidate.allowed_paths.push(name);
    }
    let result = f.freeze().unwrap();
    assert_eq!(result.files.len(), 512);
    assert_eq!(result.total_bytes, 514);
    revalidate_frozen(&f.manifest, &result).unwrap();
}

#[test]
fn present_empty_file_has_actual_empty_sha256_and_is_not_missing() {
    let f = Fixture::new();
    std::fs::write(f.maker().join("entry.py"), b"").unwrap();
    let result = f.freeze().unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.total_bytes, 0);
    assert_eq!(
        result.files[0].file_digest.as_str(),
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    revalidate_frozen(&f.manifest, &result).unwrap();
}

#[test]
fn narrowed_per_file_limit_refuses_before_copying_that_file() {
    let mut f = Fixture::new();
    f.manifest.candidate.max_file_bytes = 2;
    assert_eq!(
        f.freeze().unwrap_err().reason,
        AcceptanceReasonV1::FreezeInvalid
    );
    assert!(!f.candidate().join("entry.py").exists());
}
