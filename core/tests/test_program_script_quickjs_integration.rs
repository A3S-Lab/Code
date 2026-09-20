use a3s_code_core::tools::ToolExecutor;
use serde_json::json;

#[tokio::test]
async fn program_script_runs_embedded_quickjs_vm_with_workspace_js_path_and_ctx_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src");
    std::fs::write(
        src_dir.join("auth.rs"),
        r#"
pub struct PermissionPolicy {
    pub default_decision: String,
}
"#,
    )
    .expect("write auth source");
    std::fs::create_dir_all(dir.path().join("scripts/ptc")).expect("create scripts");
    std::fs::write(
        dir.path().join("scripts/ptc/search-auth.js"),
        r#"
export default async function run(ctx, inputs) {
  const hits = await ctx.grep(inputs.query, { path: "src", glob: "*.rs" });
  const filesText = await ctx.glob("src/**/*.rs");
  const files = filesText.split("\n").map((line) => line.trim()).filter(Boolean);
  const evidence = [];

  for (const file of files) {
    const content = await ctx.readFile(file);
    if (content.includes(inputs.query)) {
      evidence.push({
        file,
        hasDefaultDecision: content.includes("default_decision"),
      });
    }
  }

  return {
    summary: `found ${evidence.length} matching file(s)`,
    hits,
    evidence,
  };
}
"#,
    )
    .expect("write ptc script");

    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/ptc/search-auth.js",
                "language": "javascript",
                "inputs": { "query": "PermissionPolicy" },
                "allowed_tools": ["grep", "glob", "read"],
                "limits": {
                    "timeoutMs": 30000,
                    "maxToolCalls": 10,
                    "maxOutputBytes": 65536
                }
            }),
        )
        .await
        .expect("program tool execution");

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("Program script completed."));
    assert!(result.output.contains("found 1 matching file(s)"));
    assert!(result.output.contains("src/auth.rs"));
    assert!(result.output.contains("hasDefaultDecision"));

    let metadata = result.metadata.expect("program metadata");
    assert_eq!(metadata["program"]["name"], "script");
    assert_eq!(metadata["program"]["language"], "javascript");
    assert_eq!(metadata["program"]["runtime"], "embedded-quickjs");
    let tool_calls = metadata["program"]["tool_calls"].as_array().unwrap();
    assert!(tool_calls.len() >= 3);
    let tool_names = tool_calls
        .iter()
        .filter_map(|call| call["tool_name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names.iter().filter(|name| **name == "search").count(),
        2
    );
    assert!(tool_names.contains(&"read"));
    assert_eq!(
        metadata["script_result"]["evidence"][0]["hasDefaultDecision"],
        true
    );
}

#[tokio::test]
async fn program_script_times_out_without_writing_the_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("scripts");
    std::fs::write(
        dir.path().join("scripts/loop.js"),
        r#"
export default async function run() {
  while (true) {}
}
"#,
    )
    .expect("write loop");

    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let started = std::time::Instant::now();
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/loop.js",
                "language": "javascript",
                "limits": { "timeoutMs": 200 }
            }),
        )
        .await
        .expect("program tool returns a tool result");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "timeout must abort the VM, elapsed {:?}",
        started.elapsed()
    );
    assert_ne!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("timed out after 200 ms"),
        "{}",
        result.output
    );
    assert!(!dir.path().join("leaked.txt").exists());
}

#[tokio::test]
async fn program_script_cannot_read_outside_the_workspace() {
    let parent = tempfile::tempdir().expect("parent");
    let workspace = parent.path().join("ws");
    std::fs::create_dir_all(workspace.join("scripts")).expect("scripts");
    std::fs::write(parent.path().join("secret.txt"), "PROGRAM-SECRET-91").expect("secret");
    std::fs::write(
        workspace.join("scripts/escape.js"),
        r#"
export default async function run(ctx) {
  return await ctx.readFile("../secret.txt");
}
"#,
    )
    .expect("write script");

    let executor = ToolExecutor::new(workspace.to_string_lossy().to_string());
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/escape.js",
                "language": "javascript",
                "allowed_tools": ["read"],
                "limits": { "timeoutMs": 5000, "maxToolCalls": 2 }
            }),
        )
        .await
        .expect("program tool returns a tool result");

    assert!(
        result
            .output
            .contains("Workspace boundary violation: path escapes workspace"),
        "typed path error missing: {}",
        result.output
    );
    assert!(
        !result.output.contains("PROGRAM-SECRET-91"),
        "outside file leaked into program output: {}",
        result.output
    );
}

#[tokio::test]
async fn program_script_cannot_dump_host_environment_or_write_directly() {
    let marker = std::env::var("PATH").unwrap_or_else(|_| "PROGRAM-ENV-MARKER".to_string());
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("scripts");
    std::fs::write(
        dir.path().join("scripts/env.js"),
        r#"
export default async function run() {
  const channels = [];
  try { channels.push(JSON.stringify(globalThis.process && globalThis.process.env)); } catch (e) { channels.push(String(e)); }
  try { channels.push(String(std.getenv("PATH"))); } catch (e) { channels.push(String(e)); }
  try { channels.push(String(os.getenv("PATH"))); } catch (e) { channels.push(String(e)); }
  try { std.writeFile("direct-write.txt", "leaked"); channels.push("wrote"); } catch (e) { channels.push(String(e)); }
  return channels.join("\n");
}
"#,
    )
    .expect("write script");

    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/env.js",
                "language": "javascript",
                "limits": { "timeoutMs": 5000 }
            }),
        )
        .await
        .expect("program tool returns a tool result");

    assert!(
        !result.output.contains(&marker),
        "host environment leaked into program output"
    );
    assert!(
        !dir.path().join("direct-write.txt").exists(),
        "program script wrote the workspace without a governed tool"
    );
}

#[tokio::test]
async fn program_script_output_is_truncated_at_the_byte_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("scripts");
    std::fs::write(
        dir.path().join("scripts/big.js"),
        r#"
export default async function run() {
  return "A".repeat(4000);
}
"#,
    )
    .expect("write script");

    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/big.js",
                "language": "javascript",
                "limits": { "timeoutMs": 5000, "maxOutputBytes": 128 }
            }),
        )
        .await
        .expect("program tool returns a tool result");

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("[output truncated]"),
        "cap must be visible: {}",
        result.output
    );
    assert!(
        result.output.len() <= 128,
        "rendered output grew past the cap: {}",
        result.output.len()
    );
    assert!(
        !result.output.contains(&"A".repeat(200)),
        "uncapped payload leaked into the tool result"
    );
    let metadata = result.metadata.expect("program metadata").to_string();
    assert!(
        !metadata.contains(&"A".repeat(200)),
        "uncapped payload leaked into program metadata"
    );
}

/// S-PG-01: 30 QuickJS runs, 10 of which hit the time bound.
///
/// Slack is four times the larger of the idle resident-set span and the
/// growth of one infinite run. It is not a fixed megabyte guess.
#[tokio::test]
#[ignore = "S-PG-01 program soak"]
async fn soak_program_script_timeout_cycles_stay_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("scripts");
    std::fs::write(
        dir.path().join("scripts/loop.js"),
        "export default async function run() { while (true) {} }\n",
    )
    .expect("loop");
    std::fs::write(
        dir.path().join("scripts/ok.js"),
        "export default async function run() { return \"ok\"; }\n",
    )
    .expect("ok");
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());

    let idle = [resident_bytes(), resident_bytes(), resident_bytes()];
    let idle_span =
        idle.iter().copied().max().expect("idle") - idle.iter().copied().min().expect("idle");
    let before_unit = resident_bytes();
    let unit = run_program(&executor, "scripts/loop.js", 200).await;
    assert_ne!(unit.exit_code, 0, "{}", unit.output);
    let unit_growth = resident_bytes().saturating_sub(before_unit);
    let slack = unit_growth.max(idle_span).saturating_mul(4);

    let start = resident_bytes();
    let mut previous = start;
    let mut stepped_up = 0usize;
    for index in 0..30 {
        let infinite = index < 10;
        let script = if infinite {
            "scripts/loop.js"
        } else {
            "scripts/ok.js"
        };
        let started = std::time::Instant::now();
        let result = run_program(&executor, script, if infinite { 200 } else { 5_000 }).await;
        if infinite {
            assert!(
                started.elapsed() < std::time::Duration::from_millis(200 + 500 + 500),
                "cycle {index} exceeded the time bound: {:?}",
                started.elapsed()
            );
            assert!(
                result.output.contains("timed out after 200 ms"),
                "cycle {index}: {}",
                result.output
            );
            assert_ne!(result.exit_code, 0);
        } else {
            assert_eq!(result.exit_code, 0, "cycle {index}: {}", result.output);
        }
        assert!(
            !dir.path().join("direct-write.txt").exists(),
            "cycle {index} wrote the workspace without a governed tool"
        );
        let now = resident_bytes();
        if now > previous.saturating_add(idle_span.max(1)) {
            stepped_up += 1;
        }
        previous = now;
    }

    assert!(
        stepped_up < 30,
        "resident set stepped up on every cycle ({stepped_up}/30)"
    );
    let end = resident_bytes();
    assert!(
        end <= start.saturating_add(slack),
        "resident set {end} exceeds start {start} plus measured slack {slack}"
    );
}

async fn run_program(
    executor: &ToolExecutor,
    path: &str,
    timeout_ms: u64,
) -> a3s_code_core::tools::ToolResult {
    executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": path,
                "language": "javascript",
                "limits": { "timeoutMs": timeout_ms }
            }),
        )
        .await
        .expect("program tool returns a tool result")
}

fn resident_bytes() -> u64 {
    #[cfg(windows)]
    {
        use std::ffi::c_void;

        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
        }

        #[link(name = "psapi")]
        extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }

        let mut counters = std::mem::MaybeUninit::<ProcessMemoryCounters>::zeroed();
        let size = u32::try_from(std::mem::size_of::<ProcessMemoryCounters>()).expect("size");
        unsafe {
            (*counters.as_mut_ptr()).cb = size;
            assert_ne!(
                GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), size),
                0,
                "GetProcessMemoryInfo failed"
            );
            counters.assume_init().working_set_size as u64
        }
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").expect("status");
        let kibibytes = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")?
                    .trim()
                    .strip_suffix("kB")?
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .expect("VmRSS");
        kibibytes * 1024
    }
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::zeroed();
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        let result = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        assert_eq!(result, libc::KERN_SUCCESS, "task_info failed");
        unsafe { info.assume_init().resident_size }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        0
    }
}
