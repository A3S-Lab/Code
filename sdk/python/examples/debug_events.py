#!/usr/bin/env python3
"""
Debug script to see what event types are being received.
"""

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from a3s_code import Agent
import json
from conftest import find_config


def main():
    config_path = find_config()
    agent = Agent.create(config_path)
    session = agent.session(".", permissive=True)

    print("Executing simple task...")

    prompt = "Use glob to list Python files in examples directory"

    stream = session.stream(prompt)

    event_types = {}
    for event in stream:
        event_type = event.event_type
        event_types[event_type] = event_types.get(event_type, 0) + 1

        # Print first occurrence of each type with details
        if event_types[event_type] == 1:
            print(f"\nFirst {event_type} event:")
            print(f"  text: {getattr(event, 'text', None)}")
            print(f"  tool_name: {getattr(event, 'tool_name', None)}")
            print(f"  tool_id: {getattr(event, 'tool_id', None)}")
            print(f"  prompt: {getattr(event, 'prompt', None)}")
            print(f"  error: {getattr(event, 'error', None)}")

    print("\n" + "=" * 80)
    print("Event type summary:")
    for event_type, count in sorted(event_types.items()):
        print(f"  {event_type}: {count}")


if __name__ == "__main__":
    main()
