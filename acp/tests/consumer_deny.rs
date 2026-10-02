use a3s_code_acp::check_admitted_tool;

#[test]
fn seam_admission_consumer_denies_tool() {
    let mut executed = false;
    let decision = check_admitted_tool(
        "always-approve",
        "bash(*)",
        "bash",
        &serde_json::json!({"command": "echo hi"}),
        || {
            executed = true;
        },
    );
    assert_eq!(decision, Err("deny"));
    assert!(!executed);
    println!("decision=deny executed=false tool=bash posture=always-approve");
}
