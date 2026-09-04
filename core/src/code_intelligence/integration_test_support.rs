use std::{path::Path, process::Command, sync::OnceLock};

pub(super) fn compile_fake_server(output: &Path) {
    static BINARY: OnceLock<Vec<u8>> = OnceLock::new();

    let binary = BINARY.get_or_init(|| {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/code_intelligence_fake_lsp.rs");
        let build_dir = tempfile::tempdir().expect("fake language server build directory");
        let binary = build_dir.path().join(if cfg!(windows) {
            "code-intelligence-fake-lsp.exe"
        } else {
            "code-intelligence-fake-lsp"
        });
        let mut command = Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg(source)
            .arg("-o")
            .arg(&binary);
        let result = crate::tools::process::output_std_with_native_gate(&mut command)
            .expect("rustc must be available while Cargo tests are running");
        assert!(
            result.status.success(),
            "failed to compile fake language server: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read(binary).expect("read compiled fake language server")
    });

    std::fs::write(output, binary).expect("write fake language server fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))
            .expect("make fake language server executable");
    }
}

pub(super) fn fixture_started_pids(log: &str) -> Vec<u32> {
    log.lines()
        .filter(|line| line.contains("\"event\":\"process_started\""))
        .filter_map(|line| {
            line.split_once("\"pid\":")?
                .1
                .trim_end_matches('}')
                .parse()
                .ok()
        })
        .collect()
}

#[cfg(unix)]
pub(super) fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
