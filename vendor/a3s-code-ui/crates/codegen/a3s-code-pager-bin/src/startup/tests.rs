#![cfg(test)]

use super::*;
use clap::Parser;

#[test]
fn interactive_launch_requires_both_terminal_streams() {
    let args = PagerArgs::try_parse_from(["a3s-code-tui"]).expect("interactive args");
    for (input, output) in [(false, false), (false, true), (true, false)] {
        let summary = non_tty_summary(&args, input, output, "9.1.2 (test-revision)")
            .expect("non-terminal summary");
        assert_eq!(summary.lines().count(), 3);
        assert!(summary.contains("Interactive TUI requires terminal input and output."));
    }
    assert!(non_tty_summary(&args, true, true, "test").is_none());
}

#[test]
fn summary_preserves_the_explicit_model_and_built_revision() {
    let args = PagerArgs::try_parse_from(["a3s-code-tui", "--model", "provider/model"])
        .expect("explicit model");
    assert_eq!(
        non_tty_summary(&args, false, false, "9.1.2 (abc123)"),
        Some("A3S Code 9.1.2 (abc123)\nmodel: provider/model\nInteractive TUI requires terminal input and output.\n".into())
    );
}

#[test]
fn headless_prompts_and_subcommands_are_not_terminal_admitted() {
    for tail in [
        vec!["--single", "hello"],
        vec!["--prompt-file", "request.txt"],
        vec!["agent"],
        vec!["version"],
        vec!["inspect"],
    ] {
        let args = PagerArgs::try_parse_from(std::iter::once("a3s-code-tui").chain(tail))
            .expect("headless args");
        assert!(non_tty_summary(&args, false, false, "test").is_none());
    }
}

#[test]
fn dashboard_and_resume_are_interactive_launches() {
    for tail in [
        vec!["dashboard"],
        vec!["--resume", "saved-id"],
        vec!["--continue"],
    ] {
        let args = PagerArgs::try_parse_from(std::iter::once("a3s-code-tui").chain(tail))
            .expect("interactive intent");
        assert!(non_tty_summary(&args, false, false, "test").is_some());
    }
}
