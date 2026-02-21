"""
Git Worktree Tool Test with Real LLM

Demonstrates the git_worktree builtin tool via the Python SDK:
1. Initialize a git repo in a temp directory
2. Direct tool calls: status, create, list, remove
3. LLM-driven: ask the agent to use git_worktree

Run with: python test_git_worktree.py
"""

import os
import subprocess
import tempfile
from pathlib import Path

from a3s_code import Agent


def find_config() -> str:
    if "A3S_CONFIG" in os.environ:
        return os.environ["A3S_CONFIG"]
    home_config = Path.home() / ".a3s" / "config.hcl"
    if home_config.exists():
        return str(home_config)
    raise FileNotFoundError("Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG")


def init_git_repo(path: str):
    """Initialize a git repo with one commit."""
    subprocess.run(["git", "init"], cwd=path, capture_output=True, check=True)
    subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=path, capture_output=True, check=True)
    subprocess.run(["git", "config", "user.name", "Test User"], cwd=path, capture_output=True, check=True)
    Path(path, "README.md").write_text("# Test Repo\n")
    subprocess.run(["git", "add", "."], cwd=path, capture_output=True, check=True)
    subprocess.run(["git", "commit", "-m", "Initial commit"], cwd=path, capture_output=True, check=True)


def main():
    config = find_config()
    print(f"Config: {config}")

    agent = Agent(config)
    print("Agent created ✓")

    with tempfile.TemporaryDirectory() as workspace:
        init_git_repo(workspace)
        print(f"Git repo initialized at: {workspace}\n")

        session = agent.session(workspace)

        # --- Test 1: Direct tool call — status ---
        print("═══ Test 1: git_worktree status ═══")
        result = session.tool("git_worktree", {"command": "status"})
        print(result.output)
        assert result.exit_code == 0, "status should succeed"
        print()

        # --- Test 2: Direct tool call — create worktree ---
        print("═══ Test 2: git_worktree create ═══")
        wt_path = os.path.join(workspace, "wt-feature-auth")
        result = session.tool("git_worktree", {
            "command": "create",
            "branch": "feature-auth",
            "path": wt_path,
        })
        print(result.output)
        assert result.exit_code == 0, f"create failed: {result.output}"
        assert os.path.exists(wt_path), "worktree directory should exist"
        print()

        # --- Test 3: Direct tool call — list ---
        print("═══ Test 3: git_worktree list ═══")
        result = session.tool("git_worktree", {"command": "list"})
        print(result.output)
        assert result.exit_code == 0
        assert "feature-auth" in result.output, "list should contain the new branch"
        print()

        # --- Test 4: LLM-driven query ---
        print("═══ Test 4: LLM-driven worktree query ═══")
        llm_result = session.send(
            "Use the git_worktree tool with command 'list' to show me all worktrees. "
            "Just show the tool output, nothing else."
        )
        print(f"LLM response:\n{llm_result.text}")
        assert llm_result.tool_calls_count > 0, "LLM should have called git_worktree"
        print()

        # --- Test 5: Direct tool call — remove ---
        print("═══ Test 5: git_worktree remove ═══")
        result = session.tool("git_worktree", {
            "command": "remove",
            "path": wt_path,
        })
        print(result.output)
        assert result.exit_code == 0, f"remove failed: {result.output}"
        assert not os.path.exists(wt_path), "worktree directory should be gone"
        print()

        # --- Test 6: Verify cleanup ---
        print("═══ Test 6: Verify cleanup ═══")
        result = session.tool("git_worktree", {"command": "list"})
        print(result.output)
        assert "feature-auth" not in result.output
        print()

    print("═══ All git_worktree tests passed ✓ ═══")


if __name__ == "__main__":
    main()
