use super::*;

fn parse(args: &[&str]) -> Result<Args> {
    parse_args_from(args.iter().map(|arg| arg.to_string()))
}

#[test]
fn a_launch_without_the_flag_stays_interactive() {
    let args = parse(&["--cwd", "/tmp", "--model", "openai/gpt-5"]).expect("args");
    assert!(args.unattended.is_none());
    assert!(parse(&[]).expect("args").unattended.is_none());
}

#[test]
fn the_unattended_flag_carries_budget_and_result_file() {
    let args = parse(&["--unattended"]).expect("args");
    assert_eq!(
        args.unattended,
        Some(Unattended::new(DEFAULT_EXECUTION_BUDGET_MS, None).expect("profile"))
    );
    let args = parse(&[
        "--unattended",
        "--max-execution-ms",
        "1000",
        "--result-file",
        "/logs/agent/a3s-code.result.json",
    ])
    .expect("args");
    assert_eq!(
        args.unattended,
        Some(
            Unattended::new(1000, Some("/logs/agent/a3s-code.result.json".into()))
                .expect("profile")
        )
    );
}

#[test]
fn the_reasoning_effort_defaults_to_max_and_takes_only_host_levels() {
    let base = || Unattended::new(DEFAULT_EXECUTION_BUDGET_MS, None).expect("profile");
    let parsed = |args: &[&str]| parse(args).expect("args").unattended;
    assert_eq!(
        parsed(&["--unattended"]),
        Some(base().with_reasoning_effort("max").expect("max"))
    );
    assert_eq!(
        parsed(&["--unattended", "--reasoning-effort", "high"]),
        Some(base().with_reasoning_effort("high").expect("high"))
    );
    assert_ne!(
        parsed(&["--unattended", "--reasoning-effort", "high"]),
        Some(base())
    );
    assert!(parse(&["--unattended", "--reasoning-effort", "extreme"]).is_err());
    assert!(parse(&["--reasoning-effort", "high"]).is_err());
}

#[test]
fn the_engineering_policy_is_named_and_unattended_only() {
    let base = || Unattended::new(DEFAULT_EXECUTION_BUDGET_MS, None).expect("profile");
    assert_eq!(
        parse(&["--unattended", "--engineering-policy", "engineering-v1"])
            .expect("args")
            .unattended,
        Some(
            base()
                .with_engineering_policy("engineering-v1")
                .expect("policy")
        )
    );
    assert!(parse(&["--unattended", "--engineering-policy", "engineering-v9"]).is_err());
    assert!(parse(&["--unattended", "--engineering-policy"]).is_err());
    assert!(parse(&["--engineering-policy", "engineering-v1"]).is_err());
}

#[test]
fn goal_continuation_is_unattended_only() {
    let base = || Unattended::new(DEFAULT_EXECUTION_BUDGET_MS, None).expect("profile");
    assert_eq!(
        parse(&["--unattended", "--goal-continuation"])
            .expect("args")
            .unattended,
        Some(base().with_goal_continuation())
    );
    assert!(parse(&["--goal-continuation"]).is_err());
}

#[test]
fn unattended_options_are_refused_without_the_flag_or_beside_studio_flags() {
    assert!(parse(&["--max-execution-ms", "1000"]).is_err());
    assert!(parse(&["--result-file", "/tmp/r.json"]).is_err());
    assert!(parse(&["--unattended", "--max-execution-ms", "0"]).is_err());
    assert!(parse(&["--unattended", "--max-execution-ms", "soon"]).is_err());
    assert!(parse(&["--unattended", "--studio-documents"]).is_err());
    assert!(parse(&["--unattended", "--plugin-store", "/tmp/store"]).is_err());
}
