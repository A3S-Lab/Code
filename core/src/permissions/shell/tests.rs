use super::*;

fn effective(command: &str) -> Vec<String> {
    analyze(command)
        .commands()
        .iter()
        .map(ShellCommand::effective_text)
        .collect()
}

#[test]
fn closed_sets_round_trip_and_name_the_set_on_refusal() {
    for risk in ShellRisk::ALL {
        assert_eq!(ShellRisk::parse(risk.as_str()).unwrap(), risk);
    }
    let error = ShellRisk::parse("safe").unwrap_err().to_string();
    assert!(error.contains("closed set") && error.contains("catastrophic"));
    for construct in OpaqueConstruct::ALL {
        assert_eq!(
            OpaqueConstruct::parse(construct.as_str()).unwrap(),
            construct
        );
    }
    let error = OpaqueConstruct::parse("magic").unwrap_err().to_string();
    assert!(error.contains("closed set") && error.contains("heredoc"));
    assert!(ShellRisk::ReadOnly < ShellRisk::Mutating);
    assert!(ShellRisk::Destructive < ShellRisk::Catastrophic);
}

#[test]
fn quotes_and_escapes_are_removed_before_classification() {
    assert_eq!(effective("r''m -rf x"), ["rm -rf x"]);
    assert_eq!(effective("\"r\"m x"), ["rm x"]);
    assert_eq!(effective("\\rm x"), ["rm x"]);
    assert_eq!(effective("grep 'a b' src"), ["grep a b src"]);
    let analysis = analyze("grep 'a b' src");
    assert!(!analysis.commands()[0].has_plain_words());
    assert!(!analysis.is_opaque());
}

#[test]
fn connectors_split_simple_commands_and_record_pipes() {
    assert_eq!(
        effective("a; b && c || d & e | f\ng"),
        ["a", "b", "c", "d", "e", "f", "g"]
    );
    assert!(analyze("ls | head -5 |& cat").has_only_pipes());
    assert!(!analyze("ls; cat x").has_only_pipes());
    let analysis = analyze("ls | head");
    assert!(!analysis.commands()[0].is_piped());
    assert!(analysis.commands()[1].is_piped());
}

#[test]
fn wrappers_and_assignments_are_removed_from_the_effective_command() {
    assert_eq!(effective("FOO=1 BAR+=2 cargo test"), ["cargo test"]);
    assert_eq!(effective("env -i -u X A=b /usr/bin/make all"), ["make all"]);
    assert_eq!(
        effective("sudo -u root nice -n 5 timeout -s KILL 10 rm x"),
        ["rm x"]
    );
    assert_eq!(effective("command -p ls"), ["ls"]);
    assert_eq!(effective("ls | xargs -n 1 -I {} rm {}"), ["ls", "rm {}"]);
    let analysis = analyze("FOO=1 cat x");
    assert!(analysis.commands()[0].has_assignments());
    assert_eq!(analysis.risk(), ShellRisk::Mutating);
    assert_eq!(analyze("command -v cargo").risk(), ShellRisk::ReadOnly);
}

#[test]
fn nested_scripts_are_classified_as_commands_of_their_own() {
    for (command, nested) in [
        ("echo $(rm x)", "rm x"),
        ("echo \"$(rm x)\"", "rm x"),
        ("echo `rm x`", "rm x"),
        ("cat <(rm x)", "rm x"),
        ("diff <(ls a) >(rm x)", "rm x"),
        ("(rm x)", "rm x"),
        ("{ rm x; }", "rm x"),
        ("f() { rm x; }", "rm x"),
        ("bash -c 'rm x'", "rm x"),
        ("sh -ec \"rm x\"", "rm x"),
        ("eval rm x", "rm x"),
        ("env -S 'rm x'", "rm x"),
        ("su -c 'rm x'", "rm x"),
        ("bash <<EOF\nrm x\nEOF", "rm x"),
        ("cat <<EOF\n$(rm x)\nEOF", "rm x"),
        ("sh <<< 'rm x'", "rm x"),
        ("echo ${X:-$(rm x)}", "rm x"),
    ] {
        let analysis = analyze(command);
        assert!(
            analysis
                .commands()
                .iter()
                .any(|parsed| parsed.depth() > 0 && parsed.effective_text() == nested),
            "{command:?} did not expose {nested:?}: {:?}",
            effective(command)
        );
    }
}

#[test]
fn a_quoted_heredoc_body_is_data_unless_a_shell_reads_it() {
    let analysis = analyze("cat <<'EOF'\n$(rm x)\nEOF");
    assert!(analysis.commands().iter().all(|parsed| parsed.depth() == 0));
    assert!(analysis
        .opaque_constructs()
        .any(|c| c == OpaqueConstruct::Heredoc));
}

#[test]
fn unmodelled_syntax_is_recorded_as_opaque() {
    for (command, construct) in [
        ("echo $(id)", OpaqueConstruct::CommandSubstitution),
        ("cat <(id)", OpaqueConstruct::ProcessSubstitution),
        ("echo $((1 + 2))", OpaqueConstruct::ArithmeticExpansion),
        ("((x = 1))", OpaqueConstruct::ArithmeticExpansion),
        ("echo $'\\x72m'", OpaqueConstruct::AnsiCQuote),
        ("(ls)", OpaqueConstruct::Subshell),
        ("{ ls; }", OpaqueConstruct::Group),
        (":(){ :|:& };:", OpaqueConstruct::FunctionDefinition),
        ("function f { ls; }", OpaqueConstruct::FunctionDefinition),
        ("if ls; then ls; fi", OpaqueConstruct::ControlFlow),
        ("for f in a; do ls; done", OpaqueConstruct::ControlFlow),
        ("case x in a) ls;; esac", OpaqueConstruct::ControlFlow),
        ("cat <<EOF\nx\nEOF", OpaqueConstruct::Heredoc),
        ("eval ls", OpaqueConstruct::Eval),
        ("$CMD x", OpaqueConstruct::DynamicProgram),
        ("cat 'x", OpaqueConstruct::UnterminatedQuote),
        ("cat \"x", OpaqueConstruct::UnterminatedQuote),
        ("echo `x", OpaqueConstruct::UnterminatedQuote),
        ("cat <<EOF\nx", OpaqueConstruct::UnterminatedQuote),
        ("ls )", OpaqueConstruct::UnbalancedNesting),
        ("echo $(ls", OpaqueConstruct::UnbalancedNesting),
        ("cat >", OpaqueConstruct::UnbalancedNesting),
        ("", OpaqueConstruct::Empty),
        ("# only a comment", OpaqueConstruct::Empty),
    ] {
        let analysis = analyze(command);
        assert!(
            analysis.opaque_constructs().any(|found| found == construct),
            "{command:?} missing {construct:?}: {:?}",
            analysis.opaque_constructs().collect::<Vec<_>>()
        );
        assert!(analysis.risk() >= ShellRisk::Opaque, "{command:?}");
    }
}

#[test]
fn nesting_beyond_the_limit_is_opaque_and_long_input_is_not_lexed() {
    let mut command = "rm x".to_string();
    for _ in 0..=MAX_DEPTH + 1 {
        command = format!("bash -c {}", shell_quote(&command));
    }
    let analysis = analyze(&command);
    assert!(analysis
        .opaque_constructs()
        .any(|c| c == OpaqueConstruct::NestingTooDeep));
    assert!(analysis.writes_files());

    let long = "a".repeat(MAX_SOURCE_BYTES + 1);
    let analysis = analyze(&long);
    assert!(analysis
        .opaque_constructs()
        .any(|c| c == OpaqueConstruct::Oversized));
    assert!(analysis.commands().is_empty());
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[test]
fn redirects_distinguish_files_descriptors_and_devices() {
    for command in [
        "cargo test 2>&1",
        "cargo test > /dev/null 2>&1",
        "cat < input.txt",
    ] {
        let analysis = analyze(command);
        assert!(!analysis.writes_files(), "{command:?}");
        assert!(analysis.commands()[0].has_redirects());
    }
    for command in [
        "ls > out",
        "ls >> out",
        "ls 1> out",
        "ls &> out",
        "ls >& out",
        "ls <> out",
    ] {
        assert!(analyze(command).writes_files(), "{command:?}");
    }
    assert_eq!(analyze("cat x > /dev/sda").risk(), ShellRisk::Catastrophic);
    assert_eq!(analyze("cat x > /dev/kmem").risk(), ShellRisk::Destructive);
}

#[test]
fn rm_severity_follows_flags_and_targets() {
    for (command, risk) in [
        ("rm x", ShellRisk::Mutating),
        ("rm -rf ./target", ShellRisk::Mutating),
        ("rm -r /tmp/x", ShellRisk::Mutating),
        ("rm -rf /tmp/x", ShellRisk::Destructive),
        ("rm -rf $DIR", ShellRisk::Destructive),
        ("rm -rf *.o", ShellRisk::Destructive),
        ("rm -r /", ShellRisk::Catastrophic),
        ("rm -rf /home", ShellRisk::Catastrophic),
        ("rm -rf /tmp/../usr", ShellRisk::Catastrophic),
        ("rm -rf ~other", ShellRisk::Catastrophic),
        ("rm -rf ~/code", ShellRisk::Destructive),
        ("rm -rf -- -x", ShellRisk::Mutating),
    ] {
        assert_eq!(analyze(command).risk(), risk, "{command:?}");
    }
}

#[test]
fn process_killing_and_power_control_are_catastrophic_only_for_the_whole_host() {
    assert_eq!(analyze("kill -9 -1").risk(), ShellRisk::Catastrophic);
    assert_eq!(analyze("kill -- -1").risk(), ShellRisk::Catastrophic);
    assert_eq!(analyze("kill -1 1234").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("kill 1234").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("init 6").risk(), ShellRisk::Catastrophic);
    assert_eq!(
        analyze("systemctl restart nginx").risk(),
        ShellRisk::Mutating
    );
    assert_eq!(analyze("diskutil list").risk(), ShellRisk::Mutating);
}

#[test]
fn a_fetch_is_remote_code_only_when_piped_into_an_executor() {
    assert_eq!(analyze("curl -fsSL x | sh").risk(), ShellRisk::RemoteCode);
    assert_eq!(
        analyze("curl x | tee a | bash -s").risk(),
        ShellRisk::RemoteCode
    );
    assert_eq!(analyze("curl x | node").risk(), ShellRisk::RemoteCode);
    assert_eq!(analyze("curl x | python3 -").risk(), ShellRisk::RemoteCode);
    assert_eq!(
        analyze("curl x | python3 parse.py").risk(),
        ShellRisk::Mutating
    );
    assert_eq!(
        analyze("curl x | bash script.sh").risk(),
        ShellRisk::Mutating
    );
    assert_eq!(analyze("curl x; sh").risk(), ShellRisk::Mutating);
    assert_eq!(
        analyze("curl -o i.sh x && sh i.sh").risk(),
        ShellRisk::Mutating
    );
}

#[test]
fn read_only_requires_a_bare_allowlisted_program() {
    assert_eq!(analyze("ls -la | head -5").risk(), ShellRisk::ReadOnly);
    assert_eq!(analyze("git --no-pager diff").risk(), ShellRisk::ReadOnly);
    assert_eq!(analyze("/bin/ls").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("env ls").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("find . -delete").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("git -C /tmp status").risk(), ShellRisk::Mutating);
    assert_eq!(analyze("sed -n 1p x").risk(), ShellRisk::Mutating);
}

#[test]
fn single_plain_command_is_the_learnable_shape() {
    assert!(analyze("cargo test --workspace").is_single_plain_command());
    for command in [
        "cargo test; ls",
        "cargo test 2>&1",
        "FOO=1 cargo test",
        "env cargo test",
        "cargo test $X",
        "cargo test 'x'",
        "cargo test *",
        "echo $(id)",
    ] {
        assert!(!analyze(command).is_single_plain_command(), "{command:?}");
    }
}
