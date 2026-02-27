#!/usr/bin/env python3
"""
A3S Code Python SDK - MCP Servers Integration Tests

Tests:
1. Global MCP servers loaded from config.hcl
2. Session-level dynamic MCP server injection
3. Both global + session MCP servers working together
4. LLM autonomously selecting and calling MCP tools
5. Disabled MCP server is skipped

Run with: python examples/test_mcp_servers.py
"""

import asyncio
import os
import tempfile
from pathlib import Path
from a3s_code import Agent


CONFIG_PATH = str(
    Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl"
)


def truncate(text, max_len=300):
    if not text or len(text) <= max_len:
        return text
    return f"{text[:max_len]}... (truncated)"


# ── Test 1: Global MCP servers from config.hcl ──────────────────────────────

async def test_global_mcp_from_config():
    """
    Write a temp config that extends the real config with a global mcp_servers block,
    then verify the agent auto-loads it and the LLM can call the tool.
    """
    print("\n🌐 Test 1: Global MCP Servers from config.hcl")
    print("-" * 80)

    # Read the real config and append an mcp_servers block
    real_config = Path(CONFIG_PATH).read_text()
    config_with_mcp = real_config + """
mcp_servers {
    name      = "everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
"""

    with tempfile.NamedTemporaryFile(mode="w", suffix=".hcl", delete=False) as f:
        f.write(config_with_mcp)
        tmp_path = f.name

    try:
        agent = Agent.create(tmp_path)
        session = agent.session(".")

        print("Asking agent to use the MCP 'echo' tool autonomously...")
        result = session.send(
            "Use the mcp__everything__echo tool to echo the message 'Hello from global MCP!'"
        )
        print(f"✓ Response: {truncate(result.text)}")

        assert "Hello from global MCP" in result.text or "echo" in result.text.lower(), \
            f"Expected echo result, got: {result.text}"

        print("\n✅ Test 1 passed: Global MCP servers auto-loaded from config")
    finally:
        os.unlink(tmp_path)


# ── Test 2: Session-level dynamic MCP injection ──────────────────────────────

async def test_session_level_mcp_injection():
    """
    Verifies that add_mcp_server() on a live session registers tools
    immediately and the LLM can call them in the same session.
    """
    print("\n🔌 Test 2: Session-level Dynamic MCP Injection")
    print("-" * 80)

    agent = Agent.create(CONFIG_PATH)
    session = agent.session(".")

    print("Injecting MCP server dynamically into live session...")
    tool_count = session.add_mcp_server(
        name="everything",
        command="npx",
        args=["-y", "@modelcontextprotocol/server-everything", "stdio"],
    )
    print(f"✓ Registered {tool_count} tools from 'everything' server")
    assert tool_count > 0, "Expected at least 1 tool from server-everything"

    print("\nAsking agent to use the dynamically injected MCP tool...")
    result = session.send(
        "Use the mcp__everything__add tool to add the numbers 17 and 25"
    )
    print(f"✓ Response: {truncate(result.text)}")

    assert "42" in result.text, f"Expected 42 (17+25), got: {result.text}"

    print("\n✅ Test 2 passed: Session-level MCP injection works")


# ── Test 3: Global + Session MCP working together ────────────────────────────

async def test_global_and_session_mcp_together():
    """
    Verifies that global MCP servers (from config) and session-level
    MCP servers (from add_mcp_server) are both available simultaneously.
    """
    print("\n🔗 Test 3: Global + Session MCP Together")
    print("-" * 80)

    real_config = Path(CONFIG_PATH).read_text()
    config_with_global = real_config + """
mcp_servers {
    name      = "global-everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
"""

    with tempfile.NamedTemporaryFile(mode="w", suffix=".hcl", delete=False) as f:
        f.write(config_with_global)
        tmp_path = f.name

    try:
        agent = Agent.create(tmp_path)
        session = agent.session(".")

        print("Injecting additional session-level MCP server...")
        tool_count = session.add_mcp_server(
            name="session-everything",
            command="npx",
            args=["-y", "@modelcontextprotocol/server-everything", "stdio"],
        )
        print(f"✓ Session server registered {tool_count} tools")

        print("\nAsking agent to use tools from BOTH servers...")
        result = session.send(
            "First use mcp__global-everything__echo to echo 'from global', "
            "then use mcp__session-everything__echo to echo 'from session'. "
            "Report both results."
        )
        print(f"✓ Response: {truncate(result.text)}")

        lower = result.text.lower()
        assert "global" in lower or "session" in lower, \
            f"Expected both MCP results, got: {result.text}"

        print("\n✅ Test 3 passed: Global and session MCP servers work together")
    finally:
        os.unlink(tmp_path)


# ── Test 4: LLM autonomously selects MCP tool ────────────────────────────────

async def test_llm_autonomous_mcp_selection():
    """
    Verifies that the LLM can autonomously decide to use an MCP tool
    without being explicitly told which tool to call.
    """
    print("\n🤖 Test 4: LLM Autonomous MCP Tool Selection")
    print("-" * 80)

    real_config = Path(CONFIG_PATH).read_text()
    config_with_mcp = real_config + """
mcp_servers {
    name      = "everything"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = true
}
"""

    with tempfile.NamedTemporaryFile(mode="w", suffix=".hcl", delete=False) as f:
        f.write(config_with_mcp)
        tmp_path = f.name

    try:
        agent = Agent.create(tmp_path)
        session = agent.session(".")

        print("Asking agent to add numbers (without specifying which tool)...")
        result = session.send(
            "What is 123 plus 456? Use the available MCP tools to compute this."
        )
        print(f"✓ Response: {truncate(result.text)}")

        assert "579" in result.text, f"Expected 579 (123+456), got: {result.text}"

        print("\n✅ Test 4 passed: LLM autonomously selected and called MCP tool")
    finally:
        os.unlink(tmp_path)


# ── Test 5: Disabled MCP server is skipped ───────────────────────────────────

async def test_disabled_mcp_server_skipped():
    """
    Verifies that mcp_servers with enabled = false are not connected.
    """
    print("\n🚫 Test 5: Disabled MCP Server is Skipped")
    print("-" * 80)

    real_config = Path(CONFIG_PATH).read_text()
    config_with_disabled = real_config + """
mcp_servers {
    name      = "disabled-server"
    transport = "stdio"
    command   = "npx"
    args      = ["-y", "@modelcontextprotocol/server-everything", "stdio"]
    enabled   = false
}
"""

    with tempfile.NamedTemporaryFile(mode="w", suffix=".hcl", delete=False) as f:
        f.write(config_with_disabled)
        tmp_path = f.name

    try:
        agent = Agent.create(tmp_path)
        session = agent.session(".")

        print("Asking agent about available MCP tools...")
        result = session.send(
            "List all available tools that start with 'mcp__'. "
            "If there are none, say 'no MCP tools available'."
        )
        print(f"✓ Response: {truncate(result.text)}")

        assert "mcp__disabled" not in result.text.lower(), \
            f"Disabled server should not register tools, got: {result.text}"

        print("\n✅ Test 5 passed: Disabled MCP server correctly skipped")
    finally:
        os.unlink(tmp_path)


# ── Main ──────────────────────────────────────────────────────────────────────

async def main():
    print("🚀 A3S Code Python SDK - MCP Servers Integration Tests\n")
    print("=" * 80)
    print(f"📄 Using config: {CONFIG_PATH}")
    print("=" * 80)

    if not Path(CONFIG_PATH).exists():
        print(f"❌ Config not found: {CONFIG_PATH}")
        return

    tests = [
        test_global_mcp_from_config,
        test_session_level_mcp_injection,
        test_global_and_session_mcp_together,
        test_llm_autonomous_mcp_selection,
        test_disabled_mcp_server_skipped,
    ]

    passed = 0
    failed = 0
    for test in tests:
        try:
            await test()
            passed += 1
        except Exception as e:
            print(f"\n❌ FAILED: {e}")
            failed += 1

    print("\n\n" + "=" * 80)
    if failed == 0:
        print(f"✅ All {passed} MCP integration tests completed successfully!")
    else:
        print(f"⚠️  {passed} passed, {failed} failed")
    print("=" * 80)


if __name__ == "__main__":
    asyncio.run(main())
