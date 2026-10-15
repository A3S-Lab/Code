use std::path::Path;
use std::process::Command;

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn short_commit(value: &str) -> Option<String> {
    let value = value.trim();
    let hex = value
        .chars()
        .take(12)
        .collect::<String>();
    (hex.len() == 12 && hex.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(hex)
}

/// Code revision stamped when this pager is built inside a parent git tree.
///
/// A standalone Code checkout stamps `git rev-parse HEAD`. Nested under the OS
/// repo, that command walks up to the parent HEAD, which is not the Code tag
/// the CLI launch was synced from.
fn pinned_code_commit(manifest_dir: &Path) -> Option<String> {
    let code_root = manifest_dir.ancestors().nth(5)?;
    let pin = code_root.join("CODE_REVISION");
    println!("cargo:rerun-if-changed={}", pin.display());
    let text = std::fs::read_to_string(&pin).ok()?;
    short_commit(&text)
}

fn main() {
    println!("cargo:rerun-if-env-changed=GROK_VERSION");
    println!("cargo:rerun-if-env-changed=GROK_COMMIT");

    // Watch the git files that change on commit/checkout so the version stamp refreshes
    // Never emit a missing path: cargo treats it as always dirty and rebuilds this crate every build
    let mut watch_paths = Vec::new();
    watch_paths.extend(git_stdout(&["rev-parse", "--git-path", "HEAD"]));
    watch_paths.extend(git_stdout(&["rev-parse", "--git-path", "logs/HEAD"]));
    if let Some(head_ref) = git_stdout(&["symbolic-ref", "-q", "HEAD"]) {
        watch_paths.extend(git_stdout(&["rev-parse", "--git-path", &head_ref]));
    }
    for path in watch_paths.iter().filter(|p| Path::new(p).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }

    let manifest_dir = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).to_path_buf();
    let code_root = manifest_dir.ancestors().nth(5).map(Path::to_path_buf);
    let git_root = git_stdout(&["rev-parse", "--show-toplevel"]).map(std::path::PathBuf::from);
    let git_is_code_repo = match (&code_root, &git_root) {
        (Some(code_root), Some(git_root)) => git_root == code_root,
        _ => false,
    };
    let commit = std::env::var("GROK_COMMIT")
        .ok()
        .and_then(|value| short_commit(&value))
        .or_else(|| {
            if git_is_code_repo {
                git_stdout(&["rev-parse", "HEAD"]).and_then(|value| short_commit(&value))
            } else {
                pinned_code_commit(&manifest_dir)
            }
        })
        .or_else(|| git_stdout(&["rev-parse", "HEAD"]).and_then(|value| short_commit(&value)))
        .unwrap_or_else(|| "unknown".to_string());

    let version = std::env::var("GROK_VERSION")
        .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| "0.0.0".to_string());

    println!("cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})");

    // grove-projfs imports `ProjectedFSLib.dll`, absent until `Client-ProjFS` is
    // enabled; a load-time import kills startup with STATUS_DLL_NOT_FOUND. Link
    // args from grove-projfs/build.rs do not propagate to this exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:ProjectedFSLib.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
