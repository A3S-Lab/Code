#!/usr/bin/env python3
"""Quick test to verify Python SDK works with real LLM"""

from a3s_code import Agent
from pathlib import Path

def find_config():
    config = Path.home() / ".a3s" / "config.hcl"
    if config.exists():
        return str(config)
    # Try project root (6 levels up from examples/quick_test.py)
    config = Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl"
    if config.exists():
        return str(config)
    raise FileNotFoundError("Config not found")

try:
    print("🧪 Quick Python SDK Test")
    print("=" * 50)

    # Create agent
    config_path = find_config()
    print(f"✓ Config found: {config_path}")

    agent = Agent.create(config_path)
    print("✓ Agent created")

    # Create session
    session = agent.session(".")
    print("✓ Session created")

    # Simple test
    result = session.send("What is 2+2? Just answer with the number.")
    print(f"✓ LLM responded: {result.text[:50]}")

    print("\n✅ Python SDK works with real LLM!")

except Exception as e:
    print(f"\n❌ Test failed: {e}")
    import traceback
    traceback.print_exc()
