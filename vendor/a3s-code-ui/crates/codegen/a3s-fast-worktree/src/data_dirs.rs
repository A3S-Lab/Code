//! Grove data directories production actually uses.
use std::path::PathBuf;
/// platform data dir, then a3s-home/grove. Deduped. Grove does not read
///
/// Grove's data dir is `$XDG_DATA_HOME/grove` when that variable is non-empty.
/// Otherwise Windows uses `%LOCALAPPDATA%\grove` or `<home>\AppData\Local\grove`.
/// Other platforms also keep `$HOME/.local/share/grove` so an XDG override does
/// not hide the default location. A `data_dir` in grove's config replaces none
/// of those candidates; it is the effective dir when set.
#[must_use]
#[allow(dead_code)]
pub fn candidate_data_dirs() -> Vec<PathBuf> {
    candidate_data_dirs_from(
        std::env::var_os("GROVE_DATA_DIR").map(PathBuf::from),
        configured_data_dir(),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        a3s_dirs::home_dir(),
        a3s_dirs::resolve_grok_home(),
        if cfg!(windows) {
            DataDirPlatform::Windows
        } else {
            DataDirPlatform::Unix
        },
    )
}
fn configured_data_dir() -> Option<PathBuf> {
    None
}
#[derive(Clone, Copy)]
enum DataDirPlatform {
    Unix,
    Windows,
}
fn candidate_data_dirs_from(
    grove_data_dir: Option<PathBuf>,
    config_data_dir: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    home: Option<PathBuf>,
    grok_home: Option<PathBuf>,
    platform: DataDirPlatform,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |path: PathBuf| {
        if !dirs.iter().any(|existing| existing == &path) {
            dirs.push(path);
        }
    };
    if let Some(path) = non_empty(grove_data_dir) {
        push(path);
    }
    if let Some(path) = non_empty(config_data_dir) {
        push(path);
    }
    let xdg = non_empty(xdg_data_home);
    if let Some(xdg) = xdg.as_ref() {
        push(xdg.join("grove"));
    }
    match platform {
        DataDirPlatform::Windows => {
            if xdg.is_none() {
                if let Some(local) = non_empty(local_app_data) {
                    push(local.join("grove"));
                } else if let Some(home) = home.as_ref() {
                    push(home.join("AppData").join("Local").join("grove"));
                }
            }
        }
        DataDirPlatform::Unix => {
            if let Some(home) = home.as_ref() {
                push(home.join(".local/share/grove"));
            }
        }
    }
    if let Some(grok_home) = non_empty(grok_home) {
        push(grok_home.join("grove"));
    }
    dirs
}
fn non_empty(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_dirs_include_each_source_once() {
        let dirs = candidate_data_dirs_from(
            Some(PathBuf::from("/env/grove")),
            None,
            Some(PathBuf::from("/xdg")),
            None,
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("/grok")),
            DataDirPlatform::Unix,
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/env/grove"),
                PathBuf::from("/xdg/grove"),
                PathBuf::from("/home/user/.local/share/grove"),
                PathBuf::from("/grok/grove"),
            ]
        );
    }
    #[test]
    fn candidate_dirs_dedup_xdg_that_matches_home() {
        let home = PathBuf::from("/home/user");
        let dirs = candidate_data_dirs_from(
            None,
            None,
            Some(home.join(".local/share")),
            None,
            Some(home),
            None,
            DataDirPlatform::Unix,
        );
        assert_eq!(dirs, vec![PathBuf::from("/home/user/.local/share/grove")]);
    }
    #[test]
    fn windows_candidates_use_localappdata_not_unix_share() {
        let local = PathBuf::from(r"C:\AppData\Local");
        let home = PathBuf::from(r"C:\Profiles\user");
        let dirs = candidate_data_dirs_from(
            None,
            None,
            None,
            Some(local.clone()),
            Some(home.clone()),
            None,
            DataDirPlatform::Windows,
        );
        let unix_share = home.join(".local/share/grove");
        assert_eq!(vec![local.join("grove")], dirs);
        assert_eq!(None, dirs.iter().find(|path| *path == &unix_share));
    }
    #[test]
    fn windows_xdg_data_home_wins_over_localappdata() {
        let xdg = PathBuf::from(r"D:\xdg");
        let local = PathBuf::from(r"C:\AppData\Local");
        let dirs = candidate_data_dirs_from(
            None,
            None,
            Some(xdg.clone()),
            Some(local.clone()),
            Some(PathBuf::from(r"C:\Profiles\user")),
            None,
            DataDirPlatform::Windows,
        );
        let local_grove = local.join("grove");
        assert_eq!(vec![xdg.join("grove")], dirs);
        assert_eq!(None, dirs.iter().find(|path| *path == &local_grove));
    }
    #[test]
    fn config_data_dir_is_a_candidate() {
        let dirs = candidate_data_dirs_from(
            None,
            Some(PathBuf::from("/mnt/grove-data")),
            None,
            None,
            Some(PathBuf::from("/home/user")),
            None,
            DataDirPlatform::Unix,
        );
        assert_eq!(
            vec![
                PathBuf::from("/mnt/grove-data"),
                PathBuf::from("/home/user/.local/share/grove"),
            ],
            dirs,
        );
    }
}
