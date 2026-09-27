//! Installs owned by WinGet, detected from the running executable's path.
//!
//! Nothing writes, renames, or deletes a file in a WinGet package dir: WinGet re-checks the package exe's hash and
//! refuses `upgrade`/`uninstall` without `--force` once it changes.
//! Detection reads only `current_exe()`, raw or canonical: no PATH scan, registry, PowerShell, `winget.exe`, network,
//! or cache.
//! The matcher is pure and cfg-agnostic so Linux CI covers it. It matches the package directory, never the exe file
//! name, which WinGet may keep as the downloaded file name.

use std::path::Path;

use crate::version::is_stable_channel;

pub(crate) const WINGET: &str = "winget";
pub(crate) const UPGRADE_COMMAND: &str = "winget upgrade --id A3S.GrokBuild -e";
const INSTALL_COMMAND: &str = "winget install --id A3S.GrokBuild -e";

/// WinGet names a portable package dir `<PackageIdentifier>_<SourceIdentifier>`.
const PACKAGE_DIR_PREFIX: &str = "A3S.GrokBuild_";

/// True iff three consecutive segments of `path` (split on '/' and '\', ASCII case-folded) are `WinGet`, `Packages`,
/// and `A3S.GrokBuild_<source>` with a non-empty source.
pub(crate) fn is_winget_package_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    let segments: Vec<&str> = path.split(['/', '\\']).collect();
    segments.windows(3).any(|window| match window {
        [winget, packages, package] => {
            winget.eq_ignore_ascii_case("WinGet")
                && packages.eq_ignore_ascii_case("Packages")
                && package
                    .split_at_checked(PACKAGE_DIR_PREFIX.len())
                    .is_some_and(|(prefix, source)| {
                        prefix.eq_ignore_ascii_case(PACKAGE_DIR_PREFIX) && !source.is_empty()
                    })
        }
        _ => false,
    })
}

/// The release a printed WinGet command installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target<'a> {
    /// The newest release WinGet has.
    Newest,
    /// The newest release, reinstalled even when it is already installed.
    Reinstall,
    /// One release: a `--version` pin, or the newest release an org version cap allows.
    Exact(&'a str),
}

impl Target<'_> {
    fn command(self) -> String {
        // `upgrade` refuses an older or equal version; `install --force` pins in either direction.
        match self {
            Target::Newest => UPGRADE_COMMAND.to_owned(),
            Target::Reinstall => format!("{INSTALL_COMMAND} --force"),
            Target::Exact(version) => format!("{INSTALL_COMMAND} --version {version} --force"),
        }
    }
}

/// What `a3s update` prints for a WinGet install. `channel` is the requested or configured channel, which WinGet
/// cannot honor unless it is stable.
pub(crate) fn hand_off_message(target: Target<'_>, channel: &str) -> String {
    let command = target.command();
    let channel_note = ignored_channel_note(channel);
    format!(
        "A3S Code was installed with WinGet, so WinGet manages its updates.\n\
         {channel_note}\
         Quit all running Grok sessions (`grok leader kill` stops a background leader), then run:\n  \
         {command}\n\
         Use an administrator terminal if WinGet installed Grok for all users.\n\
         New releases can take a few days to reach WinGet. \
         If WinGet does not list the version yet, try again later.\n"
    )
}

/// Why a requested or configured `channel` has no effect on a WinGet install; empty when it is stable.
pub(crate) fn ignored_channel_note(channel: &str) -> String {
    if is_stable_channel(channel) {
        String::new()
    } else {
        format!(
            "WinGet ships only the stable channel, so the {channel} channel does not apply to this install.\n"
        )
    }
}

/// Follows "A new version of A3S Code is available" in headless runs and `a3s update --check`.
pub(crate) fn update_available_note(target: Target<'_>) -> String {
    format!(
        "Installed with WinGet: quit A3S Code and run `{}` \
         (new releases can take a few days to reach WinGet).",
        target.command()
    )
}

#[cfg(test)]
#[path = "winget_tests.rs"]
mod tests;
