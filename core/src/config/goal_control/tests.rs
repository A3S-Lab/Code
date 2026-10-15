#![cfg(test)]

use crate::config::{
    host_authoritative_block, rewrite_acl_sections, withhold_untrusted_host_authoritative_blocks,
    CodeConfig, ConfigSection, LayerAcl,
};

const POLICY: &str = r#"goal_control {
    automatic_enabled = true
    max_slices = "8"
    max_no_progress_slices = "2"
    deadline_unix_ms = "1861920000000"
}"#;

#[test]
fn goal_control_is_optional_and_requires_all_fields_even_when_disabled() {
    assert!(CodeConfig::default().goal_control.is_none());
    assert!(CodeConfig::from_acl("").unwrap().goal_control.is_none());
    let config = CodeConfig::from_acl(POLICY).unwrap();
    let policy = config.goal_control.unwrap();
    assert!(policy.automatic_enabled);
    assert_eq!(policy.max_slices.get(), 8);
    assert_eq!(policy.max_no_progress_slices.get(), 2);
    assert_eq!(policy.deadline_unix_ms.get(), 1861920000000);
    let disabled = POLICY.replace("true", "false");
    assert!(
        !CodeConfig::from_acl(&disabled)
            .unwrap()
            .goal_control
            .unwrap()
            .automatic_enabled
    );
    for line in POLICY.lines().filter(|line| line.contains(" = ")) {
        assert!(
            CodeConfig::from_acl(&POLICY.replace(line, "")).is_err(),
            "{line}"
        );
        assert!(CodeConfig::from_acl(
            &disabled.replace(line.trim().replace("true", "false").as_str(), "")
        )
        .is_err());
    }
}

#[test]
fn goal_control_rejects_duplicate_unknown_and_nonliteral_values() {
    for extra in [
        "automatic_enabled = true",
        "max_slices = \"8\"",
        "max_no_progress_slices = \"99\"",
        "deadline_unix_ms = \"1\"",
        "\"max_slices\" = \"99\"",
        "token_budget = \"100\"",
        "automaticEnabled = true",
    ] {
        let source = POLICY.replacen('{', &format!("{{ {extra}\n"), 1);
        let error = CodeConfig::from_acl(&source).unwrap_err().to_string();
        assert!(error.contains("goal_control"), "{error}");
    }
    for value in ["null", "\"true\"", "1", "env(\"AUTO\")", "{}", "[]"] {
        assert!(
            CodeConfig::from_acl(&POLICY.replace("true", value)).is_err(),
            "{value}"
        );
    }
    for source in [
        format!("{POLICY}\n{POLICY}"),
        "goal_control {}".into(),
        "goal_control = null".into(),
        POLICY.replacen("goal_control", "goal_control =", 1),
        POLICY.replacen("goal_control", "goal_control :", 1),
        POLICY.replacen("goal_control", "goal_control \"label\"", 1),
        format!("goal_control {{ {POLICY} {POLICY} }}"),
        format!("goal_control {{ {POLICY} }}"),
        format!("goal_control {{ nested {{}} {POLICY} }}"),
        POLICY.trim_end_matches('}').to_string(),
        POLICY.replacen('{', "{ nested {}", 1),
        POLICY.replacen('{', "{ \"ignored\"", 1),
        POLICY.replace("max_slices = \"8\"", "max_slices = eight"),
    ] {
        assert!(CodeConfig::from_acl(&source).is_err(), "{source}");
    }
}

#[test]
fn goal_control_decimal_strings_are_exact_and_canonical() {
    for field in ["max_slices", "max_no_progress_slices", "deadline_unix_ms"] {
        let line = POLICY
            .lines()
            .find(|line| line.trim_start().starts_with(field))
            .unwrap();
        for value in [
            "1",
            "1000",
            "1001",
            "9007199254740993",
            "18446744073709551615",
        ] {
            let source = POLICY.replace(line, &format!("{field} = \"{value}\""));
            let within = field != "max_slices" || value.parse::<u64>().unwrap() <= 1000;
            assert_eq!(
                CodeConfig::from_acl(&source).is_ok(),
                within,
                "{field} {value}"
            );
        }
        for value in [
            "\"0\"",
            "\"-1\"",
            "\"+1\"",
            "\"01\"",
            "\"1.0\"",
            "\"1e3\"",
            "\" 1\"",
            "\"1 \"",
            "\"\"",
            "\"18446744073709551616\"",
            "1",
            "1.5",
            "9007199254740993",
            "1.0000000000000001",
            "null",
            "false",
            "[]",
            "{}",
            "env(\"CAP\")",
        ] {
            let source = POLICY.replace(line, &format!("{field} = {value}"));
            assert!(CodeConfig::from_acl(&source).is_err(), "{field} {value}");
        }
    }
}

#[test]
fn goal_control_layer_validation_preserves_file_boundaries() {
    let decorated = format!(
        "# goal_control {{ duplicate }}\nproviders \"goal_control\" {{ api_key = env(\"CONFIG_TEST_ONLY\") headers = {{ X_Scope = \"goal_control\" }} }}\n{POLICY}"
    );
    assert_eq!(
        CodeConfig::from_acl(&decorated).unwrap().goal_control,
        CodeConfig::from_acl(POLICY).unwrap().goal_control,
    );
    let next = POLICY.replace("\"8\"", "\"11\"");
    let config = CodeConfig::from_acl_layers(&[POLICY, &next]).unwrap();
    assert_eq!(config.goal_control.unwrap().max_slices.get(), 11);
    let duplicates = format!("{POLICY}\n{next}");
    assert!(CodeConfig::from_acl(&duplicates).is_err());
    assert!(CodeConfig::from_acl_layers(&[&duplicates, POLICY]).is_err());
    assert!(CodeConfig::from_acl_layers(&[POLICY, &duplicates]).is_err());
    assert!(
        CodeConfig::from_acl_layers(&["goal_control {", "automatic_enabled = true }"]).is_err()
    );
}

#[test]
fn goal_control_workspace_cannot_activate_enlarge_disable_or_erase_host_policy() {
    let workspace = POLICY.replace("\"8\"", "\"1000\"");
    for trusted in [
        "".to_string(),
        POLICY.into(),
        POLICY.replace("true", "false"),
    ] {
        for untrusted in [
            workspace.clone(),
            POLICY.replace("true", "false"),
            "".into(),
        ] {
            let layers = [
                LayerAcl {
                    trusted: true,
                    acl: trusted.clone(),
                },
                LayerAcl {
                    trusted: false,
                    acl: untrusted.clone(),
                },
            ];
            let mut config = CodeConfig::from_acl_layers(&[&trusted, &untrusted]).unwrap();
            withhold_untrusted_host_authoritative_blocks(&mut config, &layers);
            assert_eq!(
                config.goal_control,
                CodeConfig::from_acl(&trusted).unwrap().goal_control
            );
        }
    }
    assert_eq!(host_authoritative_block(POLICY), Some("goal_control"));
    assert_eq!(
        host_authoritative_block("memory { goal_control = \"text\" }"),
        None
    );
    assert_eq!(host_authoritative_block("# goal_control {}"), None);
    let mut config = CodeConfig::from_acl(POLICY).unwrap();
    let layers = [
        LayerAcl {
            trusted: true,
            acl: "goal_control {".into(),
        },
        LayerAcl {
            trusted: false,
            acl: POLICY.into(),
        },
    ];
    withhold_untrusted_host_authoritative_blocks(&mut config, &layers);
    assert!(config.goal_control.is_none());
}

#[test]
fn goal_control_section_roundtrip_preserves_u64_and_unrelated_acl() {
    let source = "# keep this comment\ndefault_model = \"p/m\"\nproviders \"p\" { api_key = env(\"CONFIG_TEST_ONLY\") }\n";
    let mut config = CodeConfig::from_acl(source).unwrap();
    config.goal_control = CodeConfig::from_acl(POLICY).unwrap().goal_control;
    let policy = config.goal_control.as_mut().unwrap();
    policy.max_slices = crate::goal_control::MAX_GOAL_SLICES.try_into().unwrap();
    policy.max_no_progress_slices = 9007199254740993u64.try_into().unwrap();
    policy.deadline_unix_ms = u64::MAX.try_into().unwrap();
    let rendered = rewrite_acl_sections(source, &config, &[ConfigSection::GoalControl]).unwrap();
    assert!(rendered.starts_with(source));
    assert_eq!(
        CodeConfig::from_acl(&rendered).unwrap().goal_control,
        config.goal_control
    );
    assert_eq!(
        rewrite_acl_sections(&rendered, &config, &[ConfigSection::GoalControl]).unwrap(),
        rendered
    );
    config.goal_control = None;
    let ordinary_edit =
        rewrite_acl_sections(&rendered, &config, &[ConfigSection::DefaultModel]).unwrap();
    assert!(CodeConfig::from_acl(&ordinary_edit)
        .unwrap()
        .goal_control
        .is_some());
    let removed = rewrite_acl_sections(&rendered, &config, &[ConfigSection::GoalControl]).unwrap();
    assert!(removed.starts_with(source));
    assert!(removed[source.len()..].trim().is_empty());
    assert!(CodeConfig::from_acl(&removed)
        .unwrap()
        .goal_control
        .is_none());
}
