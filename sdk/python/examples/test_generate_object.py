#!/usr/bin/env python3
"""
Integration test: generate_object tool via A3S Code Python SDK

Tests structured JSON output using MiniMax-M2.7-highspeed model.
Config is loaded from .a3s/config.acl at runtime (never hardcoded).

Run: cd crates/code/sdk/python && python examples/test_generate_object.py
"""

import json
import os
import sys
import tempfile
import time
from pathlib import Path


def load_config() -> str:
    """Load .a3s/config.acl from the repo root."""
    script_dir = Path(__file__).resolve().parent
    candidates = [
        script_dir / "../../../../../../.a3s/config.acl",
        script_dir / "../../../../../.a3s/config.acl",
        script_dir / "../../../../.a3s/config.acl",
        script_dir / "../../../.a3s/config.acl",
    ]
    for p in candidates:
        resolved = p.resolve()
        if resolved.exists():
            return resolved.read_text()
    raise FileNotFoundError(".a3s/config.acl not found. Cannot run integration test.")


# ============================================================================
# Test runner
# ============================================================================

results: list[dict] = []


def run_test(name: str, fn):
    start = time.time()
    try:
        fn()
        elapsed = int((time.time() - start) * 1000)
        results.append({"name": name, "passed": True, "ms": elapsed})
        print(f"  ✓ PASS ({elapsed}ms): {name}")
    except Exception as e:
        elapsed = int((time.time() - start) * 1000)
        results.append({"name": name, "passed": False, "error": str(e), "ms": elapsed})
        print(f"  ✗ FAIL ({elapsed}ms): {name}")
        print(f"    Error: {e}")


# ============================================================================
# Tests
# ============================================================================

def main():
    print("=" * 70)
    print("generate_object Integration Test (Python SDK + MiniMax)")
    print("=" * 70)

    from a3s_code import Agent, SessionOptions, PermissionPolicy

    config_content = load_config()
    workspace = tempfile.mkdtemp(prefix="a3s-genobj-py-")
    print(f"Workspace: {workspace}\n")

    agent = Agent.create(config_content)
    policy = PermissionPolicy(default_decision="allow")
    opts = SessionOptions()
    opts.permission_policy = policy
    session = agent.session(workspace, opts)

    # ── Test 1: Simple extraction ────────────────────────────────────────────

    def test_extract_person():
        result = session.tool("generate_object", {
            "schema": {
                "type": "object",
                "required": ["name", "age", "city"],
                "properties": {
                    "name": {"type": "string"},
                    "age": {"type": "integer"},
                    "city": {"type": "string"},
                },
            },
            "prompt": 'Extract: "Bob Zhang is 32 years old and lives in Beijing."',
            "schema_name": "person",
            "mode": "tool",
        })
        assert result.exit_code == 0, f"Tool failed: {result.output}"
        parsed = json.loads(result.output)
        obj = parsed["object"]
        assert obj["name"].lower() == "bob zhang", f"name: {obj['name']}"
        assert obj["age"] == 32, f"age: {obj['age']}"
        assert "beijing" in obj["city"].lower(), f"city: {obj['city']}"
        print(f"    Result: {json.dumps(obj)}")

    run_test("generate_object: extract person info", test_extract_person)

    # ── Test 2: Enum classification ──────────────────────────────────────────

    def test_sentiment():
        result = session.tool("generate_object", {
            "schema": {
                "type": "object",
                "required": ["sentiment"],
                "properties": {
                    "sentiment": {"type": "string", "enum": ["positive", "negative", "neutral"]},
                },
            },
            "prompt": 'Classify: "I absolutely love this new feature, it works perfectly!"',
            "schema_name": "sentiment",
        })
        assert result.exit_code == 0, f"Tool failed: {result.output}"
        parsed = json.loads(result.output)
        obj = parsed["object"]
        assert obj["sentiment"] == "positive", f"sentiment: {obj['sentiment']}"
        print(f"    Result: sentiment={obj['sentiment']}")

    run_test("generate_object: sentiment classification", test_sentiment)

    # ── Test 3: Nested schema ────────────────────────────────────────────────

    def test_nested():
        result = session.tool("generate_object", {
            "schema": {
                "type": "object",
                "required": ["book"],
                "properties": {
                    "book": {
                        "type": "object",
                        "required": ["title", "author", "year", "genres"],
                        "properties": {
                            "title": {"type": "string"},
                            "author": {"type": "string"},
                            "year": {"type": "integer", "minimum": 1800, "maximum": 2030},
                            "genres": {
                                "type": "array",
                                "items": {"type": "string"},
                                "minItems": 1,
                            },
                        },
                    }
                },
            },
            "prompt": 'Extract book info: "1984 by George Orwell, published 1949, genres: dystopian fiction, political fiction"',
            "schema_name": "book_info",
        })
        assert result.exit_code == 0, f"Tool failed: {result.output}"
        parsed = json.loads(result.output)
        obj = parsed["object"]["book"]
        assert "1984" in obj["title"], f"title: {obj['title']}"
        assert "orwell" in obj["author"].lower(), f"author: {obj['author']}"
        assert obj["year"] == 1949, f"year: {obj['year']}"
        assert len(obj["genres"]) >= 1, f"genres: {obj['genres']}"
        print(f"    Result: {obj['title']} by {obj['author']} ({obj['year']})")

    run_test("generate_object: nested schema (book)", test_nested)

    # ── Test 4: Array output ─────────────────────────────────────────────────

    def test_array():
        result = session.tool("generate_object", {
            "schema": {
                "type": "object",
                "required": ["colors"],
                "properties": {
                    "colors": {
                        "type": "array",
                        "minItems": 3,
                        "maxItems": 3,
                        "items": {
                            "type": "object",
                            "required": ["name", "hex"],
                            "properties": {
                                "name": {"type": "string"},
                                "hex": {"type": "string", "pattern": "^#[0-9a-fA-F]{6}$"},
                            },
                        },
                    }
                },
            },
            "prompt": "List exactly 3 colors with their hex codes: red, green, blue.",
            "schema_name": "colors",
        })
        assert result.exit_code == 0, f"Tool failed: {result.output}"
        parsed = json.loads(result.output)
        colors = parsed["object"]["colors"]
        assert len(colors) == 3, f"Expected 3 colors, got {len(colors)}"
        for c in colors:
            assert c["hex"].startswith("#"), f"Invalid hex: {c['hex']}"
            assert len(c["hex"]) == 7, f"Hex wrong length: {c['hex']}"
        print(f"    Result: {', '.join(c['name'] + '=' + c['hex'] for c in colors)}")

    run_test("generate_object: array with pattern validation", test_array)

    # ── Test 5: Prompt mode ──────────────────────────────────────────────────

    def test_prompt_mode():
        result = session.tool("generate_object", {
            "schema": {
                "type": "object",
                "required": ["result"],
                "properties": {
                    "result": {"type": "integer"},
                },
            },
            "prompt": "What is 13 * 7? Return the numeric result.",
            "schema_name": "math",
            "mode": "prompt",
        })
        assert result.exit_code == 0, f"Tool failed: {result.output}"
        parsed = json.loads(result.output)
        assert parsed["object"]["result"] == 91, f"Expected 91, got {parsed['object']['result']}"
        print(f"    Result: 13*7 = {parsed['object']['result']}")

    run_test("generate_object: prompt mode (math)", test_prompt_mode)

    # ── Test 6: Agent-driven (session.send) ──────────────────────────────────

    def test_agent_driven():
        result = session.send(
            'Use the generate_object tool to produce a JSON object with schema '
            '{"type":"object","required":["language","paradigm"],"properties":'
            '{"language":{"type":"string"},"paradigm":{"type":"string"}}} '
            'for the following: "Rust is a systems programming language with a focus on safety."'
        )
        assert result.text is not None
        # Agent should have used the tool
        print(f"    Tool calls: {result.tool_calls_count}, tokens: {result.total_tokens}")

    run_test("generate_object: agent-driven via session.send", test_agent_driven)

    # ── Summary ──────────────────────────────────────────────────────────────

    print("\n" + "=" * 70)
    passed = sum(1 for r in results if r["passed"])
    failed = sum(1 for r in results if not r["passed"])
    print(f"Results: {passed} passed, {failed} failed, {len(results)} total")
    print("=" * 70)

    # Cleanup
    import shutil
    shutil.rmtree(workspace, ignore_errors=True)

    if failed > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
