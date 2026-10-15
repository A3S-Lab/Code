#![cfg(test)]

use super::*;

const MODEL: &str = r#"
default_model = "mock/m1"
providers "mock" { api_key = "test-only" models "m1" { name = "M1" } }
"#;
const POLICY: &str = r#"goal_control {
    automatic_enabled = true
    max_slices = "3"
    max_no_progress_slices = "1"
    deadline_unix_ms = "1861920000000"
}"#;

#[test]
fn goal_control_actual_workspace_layer_cannot_activate_or_override_host() {
    env_locked(|| {
        std::env::remove_var("A3S_DEFAULT_MODEL");
        for host in [
            "".to_string(),
            POLICY.into(),
            POLICY.replace("true", "false"),
        ] {
            for workspace_policy in [
                POLICY
                    .replace("\"3\"", "\"1000\"")
                    .replace("\"1\"", "\"99\"")
                    .replace("\"1861920000000\"", "\"18446744073709551615\""),
                POLICY.replace("true", "false"),
            ] {
                let (_user, workspace) =
                    write_user_and_workspace_layers(&format!("{MODEL}\n{host}"), &workspace_policy);
                let launch = load_launch_config(workspace.path().into(), None).unwrap();
                assert_eq!(
                    launch.config.goal_control,
                    CodeConfig::from_acl(&host).unwrap().goal_control
                );
                assert_eq!(launch.model_id, "mock/m1");
                assert_eq!(launch.config.providers.len(), 1);
            }
        }
    });
}

#[test]
fn goal_control_explicit_host_file_retains_authority() {
    env_locked(|| {
        std::env::remove_var("A3S_DEFAULT_MODEL");
        let (user, workspace) =
            write_user_and_workspace_layers(MODEL, &POLICY.replace("true", "false"));
        let explicit = user.path().join("explicit.acl");
        std::fs::write(&explicit, format!("{MODEL}\n{POLICY}")).unwrap();
        let launch = load_launch_config(workspace.path().into(), Some(explicit)).unwrap();
        assert_eq!(
            launch.config.goal_control,
            CodeConfig::from_acl(POLICY).unwrap().goal_control
        );
    });
}

#[test]
fn goal_control_layer_errors_refuse_instead_of_using_account_fallback() {
    env_locked(|| {
        std::env::remove_var("A3S_DEFAULT_MODEL");
        for bad in [
            format!("{POLICY}\n{POLICY}"),
            "goal_control {}".into(),
            POLICY.replace(
                "automatic_enabled = true",
                "automatic_enabled = true automatic_enabled = false",
            ),
        ] {
            for trusted_bad in [false, true] {
                let host = if trusted_bad { &bad } else { POLICY };
                let repository = if trusted_bad { POLICY } else { &bad };
                let (_user, workspace) =
                    write_user_and_workspace_layers(&format!("{MODEL}\n{host}"), repository);
                let error = load_launch_config(workspace.path().into(), None).unwrap_err();
                assert!(error.to_string().contains("goal_control"), "{error}");
            }
        }
    });
}
