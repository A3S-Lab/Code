#![cfg(test)]

use super::*;

const POLICY: &str = r#"goal_control {
    automatic_enabled = true
    max_slices = "3"
    max_no_progress_slices = "1"
    deadline_unix_ms = "1861920000000"
}"#;

#[test]
fn goal_control_local_tui_layers_preserve_host_authority_and_reject_duplicates() {
    let _lock = env_lock();
    let _env = EnvVar::set("A3S_DEFAULT_MODEL", None);
    let home = tempfile_dir();
    let workspace = tempfile_dir();
    let (_, model) = fixture_acl();
    let mut layers = LaunchLayers {
        workspace: workspace.clone(),
        explicit_config: None,
        home: Some(home.clone()),
    };
    for host in [
        "".to_string(),
        POLICY.into(),
        POLICY.replace("true", "false"),
    ] {
        write_acl(&home.join(".a3s/config.acl"), &format!("{model}\n{host}"));
        write_acl(
            &workspace.join(".a3s/config.acl"),
            &POLICY.replace("\"3\"", "\"99\""),
        );
        let merged = merge_launch_layers(&layers).unwrap();
        assert_eq!(
            merged.config.goal_control,
            a3s_code_core::CodeConfig::from_acl(&host)
                .unwrap()
                .goal_control
        );
    }
    write_acl(
        &workspace.join(".a3s/config.acl"),
        &format!("{POLICY}\n{POLICY}"),
    );
    assert!(merge_launch_layers(&layers)
        .unwrap_err()
        .contains("goal_control"));
    let explicit = home.join("explicit.acl");
    write_acl(&explicit, &format!("{model}\n{POLICY}"));
    layers.explicit_config = Some(explicit);
    assert_eq!(
        merge_launch_layers(&layers).unwrap().config.goal_control,
        a3s_code_core::CodeConfig::from_acl(POLICY)
            .unwrap()
            .goal_control
    );
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(workspace).unwrap();
}
