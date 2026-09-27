use super::*;
use crate::permission::bash_command_splitting::try_parse_shell;

#[test]
fn filename_recovery_preserves_positions() {
    for command in [
        r#"ls -- "$FILE""#,
        r#"ls "./$FILE""#,
        r#"rg -n needle "/tmp/${NAME}.log""#,
        r#"rg -n needle -- "$LOG""#,
        r#"LOG='/tmp/log'; ls -lh "$LOG"; rg -n needle "$LOG" | head -40; rg -v ignore "$LOG" | tail -15"#,
    ] {
        let tree = try_parse_shell(command).expect("shell tree");
        let script = PermissionScript::analyze(&tree, command);
        assert!(script.is_eligible(), "{command}");
    }
    let command = r#"LOG='/tmp/rm -rf'; rg -n "ERROR" "$LOG""#;
    let tree = try_parse_shell(command).expect("shell tree");
    assert_eq!(
        vec![vec!["rg", "-n", "ERROR", r#""$LOG""#]],
        PermissionScript::analyze(&tree, command).projections(),
    );
}

#[test]
fn unsupported_shell_and_option_surfaces_remain_ineligible() {
    for command in [
        r#"ls "$LOG""#,
        r#"cat "./$LOG""#,
        r#""$CMD" "./$LOG""#,
        r#"rg "$OPTIONS" needle file"#,
        r#"rg --pre=sh needle "./$LOG""#,
        r#"LOG='--pre=sh'; rg needle "$LOG""#,
        r#"LOG=/tmp/a; rg needle "--pre=$LOG""#,
        r#"LOG=/tmp/a; LOG=/tmp/b; ls "$LOG""#,
        r#"LOG=/tmp/a | ls "$LOG""#,
        r#"export LOG=/tmp/a; ls "$LOG""#,
        r#"LOG=/tmp/a; set -a; ls "$LOG""#,
        r#"LOG=/tmp/a ls "$LOG""#,
        r#"ls "${LOG:-/tmp/a}""#,
        r#"ls "$(echo /tmp/a)""#,
        r#"LOG=/x; ls "$LOG" 2>/dev/null"#,
        r#"LOG=/x; ls "$LOG" &"#,
        r#"rg needle * "./$LOG""#,
        r#"head -n "$COUNT" ./file"#,
        r#"RIPGREP_CONFIG_PATH=/tmp/config; rg needle "$RIPGREP_CONFIG_PATH""#,
        r#"BASH_ENV=/tmp/rc; ls "$BASH_ENV""#,
        r#"LD_LIBRARY_PATH=./lib; ls "$LD_LIBRARY_PATH""#,
        r#"GCONV_PATH=/tmp/g; rg x "$GCONV_PATH""#,
    ] {
        let tree = try_parse_shell(command).expect("shell tree");
        let script = PermissionScript::analyze(&tree, command);
        assert!(!script.is_eligible(), "{command}");
    }
}
