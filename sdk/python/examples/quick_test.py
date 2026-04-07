#!/usr/bin/env python3
"""Quick test to verify Python SDK works with real LLM"""

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from a3s_code import Agent
from conftest import find_config


class QuickTest:
    """Quick smoke test for the Python SDK."""

    def __init__(self, agent: Agent, config_path: str) -> None:
        self.agent = agent
        self.config_path = config_path

    @staticmethod
    def find_config_path() -> str:
        """Find config file using unified config loader."""
        return find_config()

    def run(self) -> None:
        """Run the quick test."""
        print("🧪 Quick Python SDK Test")
        print("=" * 50)

        print(f"✓ Config found: {self.config_path}")
        print("✓ Agent created")

        # Create session
        session = self.agent.session(".")
        print("✓ Session created")

        # Simple test
        result = session.send("What is 2+2? Just answer with the number.")
        print(f"✓ LLM responded: {result.text[:50]}")

        print("\n✅ Python SDK works with real LLM!")


if __name__ == "__main__":
    try:
        config_path = QuickTest.find_config()
        agent = Agent.create(config_path)
        test = QuickTest(agent, config_path)
        test.run()
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
