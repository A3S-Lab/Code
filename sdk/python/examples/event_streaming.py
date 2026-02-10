"""
Event Streaming Example

Demonstrates:
- Subscribing to agent events
- Handling different event types
- Monitoring agent execution in real-time
- Tracking tool usage and progress
"""

import asyncio
from a3s_code import A3sClient


async def event_streaming_example():
    print("=" * 60)
    print("Event Streaming Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="event-demo",
            workspace="/tmp/event-test",
            system_prompt="You are a helpful assistant.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # Subscribe to events
        print("2. Subscribing to events...")
        print("✓ Event stream started")
        print()
        print("Events:")
        print("-" * 60)

        # Event counters
        event_counts = {}

        # Create event handler task
        async def handle_events():
            async for event in client.subscribe_events(session_id):
                event_type = event.get("type", "UNKNOWN")
                event_counts[event_type] = event_counts.get(event_type, 0) + 1

                if event_type == "EVENT_TYPE_AGENT_START":
                    print("[START] Agent started processing")

                elif event_type == "EVENT_TYPE_TURN_START":
                    print(f"[TURN] Turn {event.get('turn')} started")

                elif event_type == "EVENT_TYPE_TEXT_DELTA":
                    print(event.get("text", ""), end="", flush=True)

                elif event_type == "EVENT_TYPE_TOOL_START":
                    print(f"\n[TOOL] Executing: {event.get('tool_name')}")

                elif event_type == "EVENT_TYPE_TOOL_END":
                    print(
                        f"[TOOL] Completed: {event.get('tool_name')} "
                        f"(exit: {event.get('exit_code')})"
                    )

                elif event_type == "EVENT_TYPE_TURN_END":
                    print(f"[TURN] Turn {event.get('turn')} completed")
                    if "usage" in event:
                        usage = event["usage"]
                        print(
                            f"  Tokens: {usage.get('total_tokens')} "
                            f"(prompt: {usage.get('prompt_tokens')}, "
                            f"completion: {usage.get('completion_tokens')})"
                        )

                elif event_type == "EVENT_TYPE_AGENT_END":
                    print("\n[END] Agent completed")
                    if "usage" in event:
                        print(f"  Total tokens: {event['usage'].get('total_tokens')}")

                elif event_type == "EVENT_TYPE_ERROR":
                    print(f"[ERROR] {event.get('message')}")

                elif event_type == "EVENT_TYPE_CONTEXT_RESOLVING":
                    providers = event.get("providers", [])
                    print(f"[CONTEXT] Resolving context from {len(providers)} providers")

                elif event_type == "EVENT_TYPE_CONTEXT_RESOLVED":
                    print(
                        f"[CONTEXT] Resolved {event.get('total_items')} items "
                        f"({event.get('total_tokens')} tokens)"
                    )

                elif event_type == "EVENT_TYPE_PERMISSION_DENIED":
                    print(f"[PERMISSION] Denied: {event.get('tool_name')}")

                elif event_type == "EVENT_TYPE_CONFIRMATION_REQUIRED":
                    print(f"[HITL] Confirmation required for: {event.get('tool_name')}")

        # Start event handler
        event_task = asyncio.create_task(handle_events())

        # Trigger some activity
        print("3. Triggering agent activity...")
        await asyncio.sleep(1)

        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "List files in /tmp directory and tell me what you find",
                }
            ],
        )

        # Wait for events to complete
        await asyncio.sleep(2)

        print()
        print("-" * 60)
        print()

        # Print event summary
        print("4. Event Summary:")
        for event_type, count in sorted(
            event_counts.items(), key=lambda x: x[1], reverse=True
        ):
            print(f"  {event_type}: {count}")
        print()

        # Clean up
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Event streaming example completed! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(event_streaming_example())
