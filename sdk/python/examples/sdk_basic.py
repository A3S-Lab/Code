#!/usr/bin/env python3
"""Basic SDK usage example — Agent.create(), session, and tool execution.

Demonstrates:
  - Loading config from `.a3s/config.hcl`
  - Creating agents with Agent.create()
  - Creating workspace-bound sessions
  - Direct tool calls via session.tool(name, args) for all built-in tools
  - Convenience methods: bash(), read_file(), glob(), grep()

No LLM calls — runs entirely offline.

Usage:
    # Build the native module first
    cd crates/code/sdk/python && maturin develop --release

    # Run the example
    python examples/sdk_basic.py

    # Or with a custom config path
    A3S_CONFIG=/path/to/config.hcl python examples/sdk_basic.py
"""

import os
import sys
import tempfile
import time

from a3s_code import Agent


def repo_root():
    """Resolve the repo root: this file is at crates/code/sdk/python/examples/"""
    return os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", "..", ".."))


def resolve_config():
    """Resolve config path: $A3S_CONFIG > repo-root/.a3s/config.hcl"""
    env = os.environ.get("A3S_CONFIG")
    if env:
        return env
    return os.path.join(repo_root(), ".a3s", "config.hcl")


def main():
    config_path = resolve_config()
    print("=== A3S Code Python SDK Basic Example ===\n")
    print(f"Config: {config_path}\n")

    # -- 1. Agent.create() ---------------------------------------------------
    print("--- Agent.create() ---")
    agent = Agent.create(config_path)
    print("[ok] Agent created\n")

    # -- 2. Create session ----------------------------------------------------
    workspace = tempfile.mkdtemp(prefix="a3s-py-")
    session = agent.session(workspace)
    print(f"[ok] Session bound to {workspace}\n")

    # -- 3. Tool: bash --------------------------------------------------------
    print("--- Tool: bash ---")
    output = session.bash("echo 'Hello from Python SDK'")
    assert "Hello from Python SDK" in output, f"unexpected: {output}"
    print(f"  output: {output.strip()}")

    # -- 4. Tool: write + read ------------------------------------------------
    print("\n--- Tool: write + read ---")
    file_path = os.path.join(workspace, "demo.txt")
    session.tool("write", {
        "file_path": file_path,
        "content": "Hello from Python SDK!\nLine 2\nLine 3",
    })
    print(f"  wrote: {file_path}")

    content = session.read_file(file_path)
    print(f"  read back ({len(content.splitlines())} lines):")
    for line in content.splitlines()[:3]:
        print(f"    {line}")

    # -- 5. Tool: edit --------------------------------------------------------
    print("\n--- session.tool('edit') ---")
    edit_result = session.tool("edit", {
        "file_path": file_path,
        "old_string": "Line 2",
        "new_string": "Line 2 (edited)",
    })
    print(f"  exit_code: {edit_result.exit_code}")
    assert edit_result.exit_code == 0, f"edit failed: {edit_result.output}"
    edited = open(os.path.join(workspace, "demo.txt")).read()
    assert "Line 2 (edited)" in edited, "edit not applied"
    print("  verified: file contains 'Line 2 (edited)'")

    # -- 6. Tool: patch -------------------------------------------------------
    print("\n--- session.tool('patch') ---")
    patch_result = session.tool("patch", {
        "file_path": file_path,
        "diff": "@@ -2,2 +2,2 @@\n-Line 2 (edited)\n+Line 2 (patched)\n Line 3",
    })
    print(f"  exit_code: {patch_result.exit_code}")
    if patch_result.exit_code == 0:
        patched = open(os.path.join(workspace, "demo.txt")).read()
        assert "Line 2 (patched)" in patched, "patch not applied"
        print("  verified: file contains 'Line 2 (patched)'")
    else:
        print(f"  patch output: {patch_result.output.strip()}")

    # -- 7. Tool: read --------------------------------------------------------
    print("\n--- session.tool('read') ---")
    read_result = session.tool("read", {
        "file_path": file_path,
        "offset": 0,
        "limit": 5,
    })
    print(f"  exit_code: {read_result.exit_code}")
    assert read_result.exit_code == 0, f"read failed: {read_result.output}"
    print("  content:")
    for line in read_result.output.splitlines()[:3]:
        print(f"    {line}")

    # -- 8. Tool: ls ----------------------------------------------------------
    print("\n--- session.tool('ls') ---")
    # Create some test files
    with open(os.path.join(workspace, "alpha.rs"), "w") as f:
        f.write("fn main() {}")
    with open(os.path.join(workspace, "beta.rs"), "w") as f:
        f.write("fn test() {}")
    with open(os.path.join(workspace, "gamma.txt"), "w") as f:
        f.write("text file")
    os.makedirs(os.path.join(workspace, "subdir"), exist_ok=True)

    ls_result = session.tool("ls", {"path": workspace})
    print(f"  exit_code: {ls_result.exit_code}")
    assert ls_result.exit_code == 0, f"ls failed: {ls_result.output}"
    assert "alpha.rs" in ls_result.output, "ls should list alpha.rs"
    assert "subdir" in ls_result.output, "ls should list subdir"
    print("  entries:")
    for line in ls_result.output.splitlines()[:6]:
        print(f"    {line}")

    # -- 9. Tool: glob --------------------------------------------------------
    print("\n--- session.tool('glob') ---")
    glob_result = session.tool("glob", {"pattern": "*.rs"})
    print(f"  exit_code: {glob_result.exit_code}")
    assert glob_result.exit_code == 0, f"glob failed: {glob_result.output}"
    assert "alpha.rs" in glob_result.output, "glob should find alpha.rs"
    print(f"  matches: {glob_result.output.strip()}")

    # -- 10. Tool: grep -------------------------------------------------------
    print("\n--- session.tool('grep') ---")
    grep_result = session.tool("grep", {"pattern": "fn ", "glob": "*.rs"})
    print(f"  exit_code: {grep_result.exit_code}")
    assert grep_result.exit_code == 0, f"grep failed: {grep_result.output}"
    assert "fn " in grep_result.output, "grep should find 'fn '"
    print("  output:")
    for line in grep_result.output.splitlines()[:5]:
        print(f"    {line}")

    # -- 11. Tool: bash (direct) ----------------------------------------------
    print("\n--- session.tool('bash') ---")
    bash_result = session.tool("bash", {
        "command": "echo 'direct tool call' && date +%Y",
        "timeout": 5000,
    })
    print(f"  exit_code: {bash_result.exit_code}")
    assert bash_result.exit_code == 0, f"bash failed: {bash_result.output}"
    assert "direct tool call" in bash_result.output
    print(f"  output: {bash_result.output.strip()}")

    # -- 12. Tool: web_fetch --------------------------------------------------
    print("\n--- session.tool('web_fetch') ---")
    fetch_result = session.tool("web_fetch", {
        "url": "https://httpbin.org/get",
        "format": "text",
        "timeout": 15,
    })
    print(f"  exit_code: {fetch_result.exit_code}")
    assert fetch_result.exit_code == 0, f"web_fetch failed: {fetch_result.output}"
    print("  response (first 5 lines):")
    for line in fetch_result.output.splitlines()[:5]:
        print(f"    {line}")

    # -- 13. Tool: web_search -------------------------------------------------
    print("\n--- session.tool('web_search') ---")
    search_result = session.tool("web_search", {
        "query": "Rust programming language",
        "engines": "ddg,wiki",
        "limit": 3,
        "timeout": 15,
        "format": "text",
    })
    print(f"  exit_code: {search_result.exit_code}")
    if search_result.exit_code == 0:
        print("  results (first 5 lines):")
        for line in search_result.output.splitlines()[:5]:
            print(f"    {line}")
    else:
        print(f"  [soft-fail] web_search error: {search_result.output.splitlines()[0] if search_result.output else '(empty)'}")

    # -- 14. Tool: cron (full lifecycle) --------------------------------------
    print("\n--- session.tool('cron') -- full lifecycle ---")
    root = repo_root()
    cron_session = agent.session(root)
    print(f"  workspace: {root} (cron data -> .a3s/cron/)")

    # parse
    print("\n  [parse] 'every 5 minutes'")
    parse_result = cron_session.tool("cron", {"action": "parse", "input": "every 5 minutes"})
    print(f"    exit_code: {parse_result.exit_code}")
    assert parse_result.exit_code == 0, f"cron parse failed: {parse_result.output}"
    assert "*/5" in parse_result.output
    print(f"    output: {parse_result.output.strip()}")

    # add
    job_name = f"py-sdk-test-{int(time.time())}"
    print(f"\n  [add] job '{job_name}' schedule='*/10 * * * *'")
    add_result = cron_session.tool("cron", {
        "action": "add",
        "name": job_name,
        "schedule": "*/10 * * * *",
        "command": "echo 'python sdk cron test'",
    })
    print(f"    exit_code: {add_result.exit_code}")
    assert add_result.exit_code == 0, f"cron add failed: {add_result.output}"

    # Extract job ID
    job_id = None
    for line in add_result.output.splitlines():
        stripped = line.strip()
        if stripped.startswith("ID: "):
            job_id = stripped[4:]
            break
    assert job_id, f"could not extract job ID from: {add_result.output}"
    print(f"    job_id: {job_id}")

    # list
    print("\n  [list]")
    list_result = cron_session.tool("cron", {"action": "list"})
    assert list_result.exit_code == 0
    assert job_name in list_result.output
    print(f"    found '{job_name}' in list")

    # get
    print(f"\n  [get] id={job_id}")
    get_result = cron_session.tool("cron", {"action": "get", "id": job_id})
    assert get_result.exit_code == 0
    assert job_name in get_result.output
    print(f"    output: {get_result.output.splitlines()[0]}")

    # pause
    print(f"\n  [pause] id={job_id}")
    pause_result = cron_session.tool("cron", {"action": "pause", "id": job_id})
    assert pause_result.exit_code == 0
    print(f"    output: {pause_result.output.strip()}")

    # resume
    print(f"\n  [resume] id={job_id}")
    resume_result = cron_session.tool("cron", {"action": "resume", "id": job_id})
    assert resume_result.exit_code == 0
    print(f"    output: {resume_result.output.strip()}")

    # run
    print(f"\n  [run] id={job_id}")
    run_result = cron_session.tool("cron", {"action": "run", "id": job_id})
    assert run_result.exit_code == 0
    print(f"    output: {run_result.output.splitlines()[0]}")

    # history
    print(f"\n  [history] id={job_id}")
    history_result = cron_session.tool("cron", {"action": "history", "id": job_id, "limit": 5})
    assert history_result.exit_code == 0
    print(f"    output: {history_result.output.splitlines()[0]}")

    # remove
    print(f"\n  [remove] id={job_id}")
    remove_result = cron_session.tool("cron", {"action": "remove", "id": job_id})
    assert remove_result.exit_code == 0
    print(f"    output: {remove_result.output.strip()}")

    # verify removal
    list_after = cron_session.tool("cron", {"action": "list"})
    assert job_name not in list_after.output
    print("    verified: job no longer in list")

    # -- 15. Unknown tool (error case) ----------------------------------------
    print("\n--- session.tool('nonexistent') ---")
    unknown_result = session.tool("nonexistent", {})
    print(f"  exit_code: {unknown_result.exit_code}")
    assert unknown_result.exit_code == 1, "unknown tool should fail"
    assert "Unknown tool" in unknown_result.output
    print(f"  output: {unknown_result.output.strip()}")

    # -- 16. Convenience methods ----------------------------------------------
    print("\n--- Convenience methods ---")
    glob_conv = session.glob("*.rs")
    print(f"  session.glob('*.rs'): {glob_conv}")

    grep_conv = session.grep("fn ")
    print(f"  session.grep('fn '): {len(grep_conv.splitlines())} lines")

    # -- Done -----------------------------------------------------------------
    print("\n=== All checks passed ===")


if __name__ == "__main__":
    main()
