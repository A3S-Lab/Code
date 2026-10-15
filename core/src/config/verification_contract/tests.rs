#![cfg(test)]

use super::*;
use crate::config::{withhold_untrusted_host_authoritative_blocks, LayerAcl};
use a3s_acl::Document;

fn block(workspace: &Path, id: &str) -> Block {
    let mut block = Block {
        name: "verification_contract".into(),
        labels: Vec::new(),
        blocks: Vec::new(),
        attributes: Default::default(),
    };
    block.labels.push(id.into());
    block.attributes.insert(
        "workspace".into(),
        Value::String(workspace.to_string_lossy().into_owned()),
    );
    block
        .attributes
        .insert("command".into(), Value::String("python3 gate.py".into()));
    block.attributes.insert(
        "covers".into(),
        Value::List(vec![Value::String("src/**".into())]),
    );
    block
}

fn acl(blocks: Vec<Block>) -> String {
    a3s_acl::generate_acl(&Document { blocks })
}

#[test]
fn literal_host_contract_roundtrips_and_applies_only_to_its_workspace() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let original = block(root.path(), "检查 合同");
    let parsed = CodeConfig::from_acl(&acl(vec![original])).unwrap();
    let selected = parsed
        .verification_contracts_for_workspace(root.path())
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "检查 合同");
    assert_eq!(selected[0].command, "python3 gate.py");
    assert_eq!(selected[0].covers, ["src/**"]);
    assert!(parsed
        .verification_contracts_for_workspace(other.path())
        .unwrap()
        .is_empty());
}

#[test]
fn malformed_host_contracts_refuse_instead_of_defaulting() {
    let root = tempfile::tempdir().unwrap();
    let valid = block(root.path(), "suite");
    let mut invalid = Vec::new();
    for key in ["workspace", "command", "covers"] {
        let mut b = valid.clone();
        b.attributes.remove(key);
        invalid.push(b);
        let mut b = valid.clone();
        b.attributes.insert(key.into(), Value::Bool(true));
        invalid.push(b);
    }
    let mut b = valid.clone();
    b.attributes.insert(
        "covers".into(),
        Value::List(vec![Value::String("../escape".into())]),
    );
    invalid.push(b);
    let mut b = valid.clone();
    b.attributes.insert(
        "covers".into(),
        Value::List(vec![
            Value::String("".into()),
            Value::String("src/**".into()),
        ]),
    );
    invalid.push(b);
    let mut b = valid.clone();
    b.attributes
        .insert("workspace".into(), Value::String("relative".into()));
    invalid.push(b);
    let mut b = valid.clone();
    b.attributes
        .insert("command".into(), Value::String("".into()));
    invalid.push(b);
    let mut b = valid.clone();
    b.attributes
        .insert("grant_verified".into(), Value::Bool(true));
    invalid.push(b);
    let mut b = valid.clone();
    b.blocks.push(Block {
        name: "override".into(),
        labels: Vec::new(),
        blocks: Vec::new(),
        attributes: Default::default(),
    });
    invalid.push(b);
    let mut b = valid.clone();
    b.labels.clear();
    invalid.push(b);
    let mut b = valid.clone();
    b.labels.push("extra".into());
    invalid.push(b);
    for b in invalid {
        assert!(CodeConfig::from_acl(&acl(vec![b])).is_err());
    }
    assert!(CodeConfig::from_acl(&acl(vec![valid.clone(), valid])).is_err());
}

#[test]
fn workspace_layer_cannot_add_or_replace_host_verification_authority() {
    let root = tempfile::tempdir().unwrap();
    let trusted = acl(vec![block(root.path(), "host")]);
    let untrusted = acl(vec![block(root.path(), "attacker")]);
    let layers = vec![
        LayerAcl {
            trusted: true,
            acl: trusted.clone(),
        },
        LayerAcl {
            trusted: false,
            acl: untrusted.clone(),
        },
    ];
    let mut parsed = CodeConfig::from_acl(&format!("{trusted}\n{untrusted}")).unwrap();
    assert_eq!(parsed.verification_contracts.len(), 2);
    assert_eq!(
        withhold_untrusted_host_authoritative_blocks(&mut parsed, &layers),
        ["verification_contract"]
    );
    assert_eq!(parsed.verification_contracts.len(), 1);
    assert_eq!(parsed.verification_contracts[0].contract.id, "host");
    let mut only_workspace = CodeConfig::from_acl(&untrusted).unwrap();
    withhold_untrusted_host_authoritative_blocks(&mut only_workspace, &layers[1..]);
    assert!(only_workspace.verification_contracts.is_empty());
}

#[test]
fn missing_or_nondirectory_declared_workspace_refuses_activation() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let parsed = CodeConfig::from_acl(&acl(vec![block(&missing, "suite")])).unwrap();
    assert!(parsed
        .verification_contracts_for_workspace(root.path())
        .is_err());
    let file = root.path().join("file");
    std::fs::write(&file, "fixture").unwrap();
    let parsed = CodeConfig::from_acl(&acl(vec![block(&file, "suite")])).unwrap();
    assert!(parsed
        .verification_contracts_for_workspace(root.path())
        .is_err());
}

#[cfg(unix)]
#[test]
fn aliased_workspace_cannot_hide_duplicate_contract_ids() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&workspace, &alias).unwrap();
    let parsed = CodeConfig::from_acl(&acl(vec![
        block(&workspace, "suite"),
        block(&alias, "suite"),
    ]))
    .unwrap();
    assert!(parsed
        .verification_contracts_for_workspace(&workspace)
        .is_err());
    let parsed = CodeConfig::from_acl(&acl(vec![block(&alias, "suite")])).unwrap();
    assert_eq!(
        parsed
            .verification_contracts_for_workspace(&workspace)
            .unwrap()
            .len(),
        1
    );
}
