#!/usr/bin/env python3
"""LLM chat example — sends a prompt and streams the response.

Demonstrates:
  - Loading config from `.a3s/config.hcl`
  - Creating an agent via Agent.create()
  - Non-streaming session.send() call
  - Streaming session.stream() with event iteration
  - Model override via session(workspace, model=...)

Requires network access and valid API keys in config.

Usage:
    # Build the native module first
    cd crates/code/sdk/python && maturin develop --release

    # Run the example
    python examples/sdk_chat.py

    # With a custom prompt
    python examples/sdk_chat.py "Explain Rust's ownership model in 3 sentences"
"""

import os
import sys

from a3s_code import Agent


def resolve_config():
    env = os.environ.get("A3S_CONFIG")
    if env:
        return env
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", "..", ".."))
    return os.path.join(root, ".a3s", "config.hcl")


def main():
    prompt = " ".join(sys.argv[1:]) if len(sys.argv) > 1 else "What is 2+2? Reply with just the number and nothing else."

    config_path = resolve_config()
    print("=== A3S Code Python SDK Chat Example ===\n")
    print(f"Config: {config_path}")
    print(f"Prompt: {prompt}\n")

    # -- 1. Create agent ------------------------------------------------------
    agent = Agent.create(config_path)
    print("[ok] Agent created\n")

    # -- 2. Non-streaming call ------------------------------------------------
    print("--- Non-streaming (session.send) ---")
    import tempfile
    workspace = tempfile.mkdtemp(prefix="a3s-py-chat-")
    session = agent.session(workspace)

    result = session.send(prompt)
    print(f"  Response: {result.text.strip()}")
    print(f"  Tokens: {result.prompt_tokens} in / {result.completion_tokens} out")

    # -- 3. Streaming call ----------------------------------------------------
    print("\n--- Streaming (session.stream) ---")
    workspace2 = tempfile.mkdtemp(prefix="a3s-py-stream-")
    session2 = agent.session(workspace2)

    print("  Response: ", end="", flush=True)
    collected = ""
    for event in session2.stream(prompt):
        if event.event_type == "text_delta":
            print(event.text, end="", flush=True)
            collected += event.text or ""
        elif event.event_type == "end":
            print()
            print(f"  Tokens: {event.total_tokens} total")
            break

    # -- 4. Model override session --------------------------------------------
    print("\n--- Session with model override ---")
    workspace3 = tempfile.mkdtemp(prefix="a3s-py-override-")
    try:
        override_session = agent.session(workspace3, model="anthropic/claude-sonnet-4-20250514")
        print("[ok] Session with anthropic/claude-sonnet-4-20250514 created")
        try:
            result = override_session.send(prompt)
            print(f"  Response: {result.text.strip()}")
            print(f"  Tokens: {result.prompt_tokens} in / {result.completion_tokens} out")
        except Exception as e:
            print(f"[skip] LLM call failed (rate limit / API error): {e}")
    except Exception as e:
        print(f"[skip] Model override failed (expected if model not in config): {e}")

    print("\n=== Done ===")


if __name__ == "__main__":
    main()
