use super::*;

#[test]
fn winget_package_paths_are_matched_by_directory() {
    let matching = [
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe\grok.exe",
        r"C:\Program Files\WinGet\Packages\A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe\grok.exe",
        r"c:\users\alice\appdata\local\microsoft\winget\packages\a3s.grokbuild_microsoft.winget.source_8wekyb3d8bbwe\GROK.EXE",
        "C:/Users/alice/AppData/Local/Microsoft/WinGet/Packages/A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe/grok.exe",
        r"\\?\C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe\grok.exe",
        r"\\fileserver\profiles\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe\grok-1.0.40-windows-x86_64.exe",
    ];
    let non_matching = [
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Links\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild_\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuildTools_x\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuild.Preview_x\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Packages\A3S.GrokBuil€_x\grok.exe",
        r"C:\Users\alice\AppData\Local\Microsoft\WinGet\Cache\Packages\A3S.GrokBuild_x\grok.exe",
        r"C:\Users\alice\AppData\Local\Packages\WinGet\A3S.GrokBuild_x\grok.exe",
        r"D:\tools\grok\grok.exe",
    ];
    let is_match = |path: &&str| is_winget_package_path(Path::new(path));
    let false_negatives = matching.into_iter().filter(|path| !is_match(path));
    let false_positives = non_matching.into_iter().filter(is_match);
    let misclassified: Vec<&str> = false_negatives.chain(false_positives).collect();
    assert_eq!(Vec::<&str>::new(), misclassified);
}

#[cfg(unix)]
#[test]
fn non_utf8_path_still_matches() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let bytes = b"/home/\xff/AppData/Local/Microsoft/WinGet/Packages/A3S.GrokBuild_x/grok";
    assert!(is_winget_package_path(Path::new(OsStr::from_bytes(bytes))));
}

#[test]
fn pinned_hand_off_forces_install_and_notes_ignored_channel() {
    let expected = "A3S Code was installed with WinGet, so WinGet manages its updates.\n\
        WinGet ships only the stable channel, so the alpha channel does not apply to this install.\n\
        Quit all running Grok sessions (`grok leader kill` stops a background leader), then run:\n  \
        winget install --id A3S.GrokBuild -e --version 1.0.40 --force\n\
        Use an administrator terminal if WinGet installed Grok for all users.\n\
        New releases can take a few days to reach WinGet. \
        If WinGet does not list the version yet, try again later.\n";
    assert_eq!(expected, hand_off_message(Target::Exact("1.0.40"), "alpha"));
}

#[test]
fn stable_hand_off_upgrades_without_channel_note() {
    let message = hand_off_message(Target::Newest, "stable");
    assert!(
        message.contains(UPGRADE_COMMAND) && !message.contains("only the stable channel"),
        "{message}"
    );
}

#[test]
fn reinstall_hand_off_forces_install_without_a_version() {
    let message = hand_off_message(Target::Reinstall, "");
    assert!(
        message.contains("\n  winget install --id A3S.GrokBuild -e --force\n"),
        "{message}"
    );
}
