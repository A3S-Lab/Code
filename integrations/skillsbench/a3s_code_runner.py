#!/usr/bin/env python3
"""Run a single SkillsBench task with a3s-code and emit machine-readable JSON."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

from a3s_code import Agent


def _env_flag(name: str, default: bool) -> bool:
    value = os.getenv(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _env_json(name: str, default):
    value = os.getenv(name)
    if not value:
        return default
    return json.loads(value)


def _normalize_mcp_server(server: dict) -> dict:
    transport = server.get("transport", "stdio")
    if isinstance(transport, dict):
        kind = transport.get("type", "stdio")
        merged = dict(server)
        merged["transport"] = kind
        for key in ("command", "args", "url", "headers"):
            if key in transport and key not in merged:
                merged[key] = transport[key]
        return merged
    return server


def main() -> int:
    instruction = os.getenv("A3S_CODE_INSTRUCTION", "").strip()
    if not instruction:
        raise RuntimeError("A3S_CODE_INSTRUCTION is required")

    workspace = os.getenv("A3S_CODE_WORKSPACE", os.getcwd())
    config_path = os.getenv("A3S_CODE_CONFIG", "/workspace/.a3s/config.hcl")
    skill_dirs = _env_json("A3S_CODE_SKILL_DIRS_JSON", [])
    mcp_servers = _env_json("A3S_CODE_MCP_SERVERS_JSON", [])

    if not Path(config_path).exists():
        raise FileNotFoundError(f"A3S code config not found: {config_path}")

    agent = Agent.create(config_path)
    session = agent.session(
        workspace,
        builtin_skills=_env_flag("A3S_CODE_BUILTIN_SKILLS", True),
        planning=_env_flag("A3S_CODE_PLANNING", True),
        permissive=_env_flag("A3S_CODE_PERMISSIVE", True),
        skill_dirs=skill_dirs or None,
    )

    for raw_server in mcp_servers:
        server = _normalize_mcp_server(raw_server)
        session.add_mcp_server(
            server["name"],
            transport=server.get("transport", "stdio"),
            command=server.get("command"),
            args=server.get("args"),
            url=server.get("url"),
            headers=server.get("headers"),
            env=server.get("env"),
            timeout_ms=server.get("timeout_ms"),
        )

    result = session.send(instruction)

    payload = {
        "text": result.text,
        "tool_calls_count": result.tool_calls_count,
        "prompt_tokens": result.prompt_tokens,
        "completion_tokens": result.completion_tokens,
        "total_tokens": result.total_tokens,
        "workspace": workspace,
        "config_path": config_path,
        "skill_dirs": skill_dirs,
        "mcp_servers": [server.get("name") for server in mcp_servers if isinstance(server, dict)],
    }
    sys.stdout.write("A3S_CODE_RESULT_BEGIN\n")
    sys.stdout.write(json.dumps(payload, ensure_ascii=False))
    sys.stdout.write("\nA3S_CODE_RESULT_END\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # pragma: no cover
        sys.stderr.write(f"a3s-code runner failed: {exc}\n")
        raise
