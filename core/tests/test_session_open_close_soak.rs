//! S-RT-01: repeated open/close on one workspace must not leak handles or
//! isolation directories, and a later open must still succeed.

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::{Agent, SessionOptions};
use std::ffi::c_void;
use std::path::Path;

fn offline_config(sessions_dir: &Path) -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        sessions_dir: Some(sessions_dir.to_path_buf()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn open_handle_count() -> usize {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn GetProcessHandleCount(process: *mut c_void, count: *mut u32) -> i32;
        }
        let mut count = 0u32;
        let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
        assert_ne!(ok, 0, "GetProcessHandleCount failed");
        count as usize
    }
    #[cfg(unix)]
    {
        let directory = if cfg!(target_os = "linux") {
            "/proc/self/fd"
        } else {
            "/dev/fd"
        };
        std::fs::read_dir(directory)
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}

#[tokio::test]
#[ignore = "S-RT-01 soak: 200 session open/close cycles"]
async fn soak_two_hundred_session_open_close_cycles_stay_bounded() {
    let workspace = tempfile::tempdir().expect("workspace");
    let sessions = tempfile::tempdir().expect("sessions");
    let isolate = workspace
        .path()
        .parent()
        .expect("workspace parent")
        .join(".a3s-isolate-s-rt-01");
    let agent = Agent::from_config(offline_config(sessions.path()))
        .await
        .expect("agent");

    // The first open installs process-lifetime runtime handles. Later cycles
    // are the leak signal: growth must stay flat, not track the iteration count.
    let warmup = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-rt-01")
                    .with_model("anthropic/claude-sonnet-4-20250514"),
            ),
        )
        .await
        .expect("warmup open");
    warmup.close().await;
    let before = open_handle_count();

    for _ in 0..200 {
        let session = agent
            .session_async(
                workspace.path().to_string_lossy().to_string(),
                Some(
                    SessionOptions::new()
                        .with_session_id("s-rt-01")
                        .with_model("anthropic/claude-sonnet-4-20250514"),
                ),
            )
            .await
            .expect("open");
        session.close().await;
    }

    let again = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-rt-01")
                    .with_model("anthropic/claude-sonnet-4-20250514"),
            ),
        )
        .await
        .expect("open after 200 close cycles");
    again.close().await;

    let after = open_handle_count();
    assert!(
        after <= before.saturating_add(5),
        "handle count grew from {before} to {after} across 200 cycles"
    );
    assert!(
        !isolate.exists(),
        "closed session left {}",
        isolate.display()
    );
}
