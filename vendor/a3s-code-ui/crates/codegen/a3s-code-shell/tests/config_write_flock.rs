//! Regression: settings saves must serialize against config-init-flock writers, or the two
//! domains interleave read-modify-writes and the last atomic rename drops the other side's edit.

use std::path::Path;
use std::time::{Duration, Instant};

fn wait_for(deadline: Instant, mut check: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// `acquire_init_lock` opens `.config-init.lock` before its 1s `try_lock` loop.
/// A second fd means the saver is already in that loop.
fn saver_has_opened_init_lock(lock_path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return false;
    };
    let opens = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let Ok(linked) = std::fs::read_link(entry.path()) else {
                return false;
            };
            if linked == lock_path {
                return true;
            }
            match (
                std::fs::canonicalize(&linked),
                std::fs::canonicalize(lock_path),
            ) {
                (Ok(linked), Ok(lock_path)) => linked == lock_path,
                _ => false,
            }
        })
        .count();
    opens >= 2
}

#[test]
fn settings_save_serializes_against_init_flock_writer() {
    // One #[test] per binary: the env is process-global (same rule as
    // acp_harness::run_agent_test).
    let grok_home = tempfile::tempdir().expect("grok home");
    // SAFETY: no other threads are running yet.
    unsafe { std::env::set_var("GROK_HOME", grok_home.path()) };

    let config_path = grok_home.path().join("config.toml");
    std::fs::write(&config_path, "[cli]\n").unwrap();

    // Flock-holding writer mid read-modify-write (the shape of
    // session-start auto-enable's `add_enabled_plugin`).
    let flock =
        a3s_code_shell::util::config::acquire_init_lock(grok_home.path()).expect("init flock");
    let stale_read = std::fs::read_to_string(&config_path).unwrap();

    // Concurrent settings toggle on its own runtime thread.
    let saver = std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(a3s_code_shell::util::config::update_config(|cfg| {
                cfg.ui.simple_mode = Some(true);
            }))
    });

    // The saver fails closed after 1s of `try_lock` retries. A fixed sleep spends
    // that budget before this thread is scheduled to drop the flock.
    let lock_path = grok_home.path().join(".config-init.lock");
    assert!(
        wait_for(Instant::now() + Duration::from_secs(10), || {
            saver_has_opened_init_lock(&lock_path)
        }),
        "settings save never opened {}",
        lock_path.display()
    );
    let mut modified: toml::Value = toml::from_str(&stale_read).unwrap();
    modified.as_table_mut().unwrap().insert(
        "plugins".into(),
        toml::from_str("enabled = [\"demo\"]").unwrap(),
    );
    a3s_code_shell::util::config::atomic_write_string(
        &config_path,
        &toml::to_string_pretty(&modified).unwrap(),
    )
    .unwrap();
    drop(flock);

    saver
        .join()
        .expect("saver thread")
        .expect("settings save succeeds");

    let merged: toml::Value =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        merged
            .get("plugins")
            .and_then(|p| p.get("enabled"))
            .and_then(|e| e.as_array())
            .map(|a| a.len()),
        Some(1),
        "flock writer's plugin enable must survive: {merged}"
    );
    assert_eq!(
        merged
            .get("ui")
            .and_then(|u| u.get("simple_mode"))
            .and_then(|v| v.as_bool()),
        Some(true),
        "settings save must survive the flock writer's rename: {merged}"
    );
}
