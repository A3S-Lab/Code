//! Windows host-shell choice and the note that tells the model which shell runs.

/// Host shells in preference order: PowerShell 7, then the Windows PowerShell 5.1
/// that ships with every Windows 10 and 11 install.
pub(crate) fn host_powershell_candidates(
    program_files: Option<&std::path::Path>,
    system_root: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = program_files {
        candidates.push(program_files.join("PowerShell").join("7").join("pwsh.exe"));
    }
    if let Some(system_root) = system_root {
        candidates.push(
            system_root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe"),
        );
    }
    candidates
}

/// Model-facing note on the host shell. `None` means no PowerShell was found.
pub(crate) fn windows_shell_guidance(shell: Option<&std::path::Path>) -> String {
    let Some(shell) = shell else {
        return "Shell: no PowerShell was found, so the `bash` tool cannot run commands on this machine.".to_string();
    };
    let legacy = shell
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("powershell.exe"));
    let name = if legacy {
        "Windows PowerShell 5.1"
    } else {
        "PowerShell 7"
    };
    let mut note = format!(
        "Shell: the `bash` tool runs {name}, not GNU bash. Write PowerShell commands, for example \
         `Get-ChildItem -Force`, `Get-Content -TotalCount 20 file`, `Select-String -Pattern text -Path *.md`, \
         `Remove-Item -Recurse dir`, `$env:NAME = 'value'`. Bash forms such as `ls -la`, `grep`, `head`, \
         `rm -rf`, `export`, and `2>/dev/null` fail."
    );
    if legacy {
        note.push_str(
            " `&&` and `||` do not work in this version; separate commands with `;` or use `if ($?) { ... }`.",
        );
    }
    note
}
