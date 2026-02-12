"""
Observability Example

Demonstrates how to monitor agent performance and costs:
- Tool usage metrics (call counts, durations, success/failure rates)
- LLM cost tracking (per-model, per-day breakdowns)
- Using metrics for optimization decisions
"""

import asyncio
from a3s_code import A3sClient


async def observability_example():
    print("=" * 60)
    print("Observability Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session and do some work to generate metrics
        print("1. Creating session and generating activity...")
        session = await client.create_session(
            name="observability-demo",
            workspace="/tmp/observability-test",
            system_prompt="You are a helpful coding assistant.",
            llm={
                "provider": "anthropic",
                "model": "claude-sonnet-4-20250514",
            },
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")

        # Generate some activity to produce metrics
        print("  Generating responses to create metrics data...")
        await client.generate(
            session_id=session_id,
            messages=[{"role": "ROLE_USER", "content": "Write a hello world in Python"}],
        )
        await client.generate(
            session_id=session_id,
            messages=[{"role": "ROLE_USER", "content": "Now write it in Rust"}],
        )
        print("✓ Activity generated")
        print()

        # =====================================================================
        # Tool Metrics
        # =====================================================================
        print("2. Getting tool metrics (all tools)...")
        metrics = await client.get_tool_metrics(session_id=session_id)
        print(f"✓ Tool metrics:")
        print(f"  Total calls: {metrics['total_calls']}")
        print(f"  Total duration: {metrics['total_duration_ms']}ms")
        print()

        if metrics["tools"]:
            print("  Per-tool breakdown:")
            for tool in metrics["tools"]:
                success_rate = (
                    f"{tool['success_count'] / tool['call_count'] * 100:.0f}%"
                    if tool["call_count"] > 0
                    else "N/A"
                )
                print(f"  - {tool['tool_name']}:")
                print(f"      Calls: {tool['call_count']} (success: {success_rate})")
                print(f"      Duration: avg={tool['avg_duration_ms']}ms, "
                      f"min={tool['min_duration_ms']}ms, max={tool['max_duration_ms']}ms")
            print()

        # Filter by specific tool
        print("3. Getting metrics for a specific tool...")
        bash_metrics = await client.get_tool_metrics(
            session_id=session_id, tool_name="bash"
        )
        if bash_metrics["tools"]:
            tool = bash_metrics["tools"][0]
            print(f"✓ Bash tool: {tool['call_count']} calls, "
                  f"{tool['success_count']} success, {tool['failure_count']} failures")
        else:
            print("  No bash tool usage recorded")
        print()

        # =====================================================================
        # Cost Summary
        # =====================================================================
        print("4. Getting cost summary...")
        cost = await client.get_cost_summary(session_id=session_id)
        print(f"✓ Cost summary:")
        print(f"  Total cost: ${cost['total_cost_usd']:.6f}")
        print(f"  Total tokens: {cost['total_tokens']}")
        print(f"    Prompt: {cost['total_prompt_tokens']}")
        print(f"    Completion: {cost['total_completion_tokens']}")
        print(f"  API calls: {cost['call_count']}")
        print()

        # Per-model breakdown
        if cost["by_model"]:
            print("  Per-model breakdown:")
            for model in cost["by_model"]:
                print(f"  - {model['model']}:")
                print(f"      Cost: ${model['cost_usd']:.6f}")
                print(f"      Tokens: {model['prompt_tokens']} prompt + "
                      f"{model['completion_tokens']} completion")
                print(f"      Calls: {model['call_count']}")
            print()

        # Per-day breakdown
        if cost["by_day"]:
            print("  Per-day breakdown:")
            for day in cost["by_day"]:
                print(f"  - {day['date']}: ${day['cost_usd']:.6f} ({day['call_count']} calls)")
            print()

        # =====================================================================
        # Cross-session Cost Summary
        # =====================================================================
        print("5. Getting cross-session cost summary...")
        total_cost = await client.get_cost_summary()  # No session filter
        print(f"✓ All sessions:")
        print(f"  Total cost: ${total_cost['total_cost_usd']:.6f}")
        print(f"  Total calls: {total_cost['call_count']}")
        print()

        # Filter by model
        print("6. Getting cost for specific model...")
        model_cost = await client.get_cost_summary(model="claude-sonnet-4-20250514")
        print(f"✓ Claude Sonnet 4 cost: ${model_cost['total_cost_usd']:.6f}")
        print()

        # Filter by date range
        print("7. Getting cost for date range...")
        date_cost = await client.get_cost_summary(
            start_date="2025-01-01",
            end_date="2025-12-31",
        )
        print(f"✓ 2025 cost: ${date_cost['total_cost_usd']:.6f} ({date_cost['call_count']} calls)")
        print()

        # Clean up
        print("8. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Observability example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(observability_example())
