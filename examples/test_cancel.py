#!/usr/bin/env python3
"""Test cancellation API"""

import threading
import time
from a3s_code import Agent

def main():
    # Create agent from config
    agent = Agent.create("~/.a3s/config.acl")
    session = agent.session(".")

    print("Starting long-running operation...")

    # Cancel after 3 seconds
    def cancel_after_delay():
        time.sleep(3)
        print("\n🛑 Cancelling operation...")
        cancelled = session.cancel()
        print(f"Cancelled: {cancelled}")

    t = threading.Thread(target=cancel_after_delay)
    t.start()

    # Start a long operation
    try:
        result = session.send("Write a very long story about a robot learning to code. Make it at least 5000 words.")
        print("\n✅ Operation completed (possibly partial)")
        print(f"Response length: {len(result.text)} chars")
        print(f"First 200 chars: {result.text[:200]}")
    except Exception as e:
        print(f"\n❌ Operation failed: {e}")

    t.join()

if __name__ == "__main__":
    main()
