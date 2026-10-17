//! Approval learning refuses the shared shell bypass corpus: none of it can
//! be learned, so none of it can match a learned rule either.

#[path = "../../core/tests/support/shell_bypass_corpus.rs"]
mod corpus;

use a3s_code_acp::approval_learning::learnable_rule;

#[test]
fn no_bypass_in_the_corpus_is_learnable() {
    for command in corpus::CATASTROPHIC
        .iter()
        .chain(corpus::HARD_DENIAL)
        .chain(corpus::SMUGGLED)
        .chain(corpus::RM_DENY_EVASIONS)
        .chain(corpus::VERIFIER_WRITES)
    {
        assert_eq!(
            learnable_rule(command),
            None,
            "approval learning accepted {command:?}"
        );
    }
}

#[test]
fn plain_commands_still_learn_their_rule() {
    for (command, rule) in [
        ("git status", "git status"),
        ("git status --short", "git status"),
        ("cargo test --workspace", "cargo test"),
        ("ls -la", "ls"),
        ("grep 'a b' src", "grep"),
        ("npm run lint", "npm run lint"),
    ] {
        assert_eq!(
            learnable_rule(command).as_deref(),
            Some(rule),
            "{command:?}"
        );
    }
}
