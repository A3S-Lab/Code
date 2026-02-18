#!/usr/bin/env python3
"""
A3S Code Python SDK - Session Internal Parallel Processing Test

Tests the INTERNAL parallel processing capability of A3S Code Session.

When a single session.send() triggers multiple tool calls, the session's
internal queue can execute these tools in parallel based on lane configuration.

This is different from calling session.send() multiple times in parallel.

Run with: python examples/test_session_internal_parallel.py
"""

import time
from pathlib import Path
from a3s_code import Agent, SessionQueueConfig


def find_config_path():
    """Find config file."""
    config_path = Path("/Users/roylin/Desktop/ai-lab/a3s/.a3s/config.hcl")
    if config_path.exists():
        return str(config_path)

    project_config = Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl"
    if project_config.exists():
        return str(project_config)

    raise FileNotFoundError("Config file not found")


def test_internal_parallel_tool_execution():
    """
    Test Session's internal parallel tool execution.

    When LLM decides to call multiple tools, the session's queue
    can execute them in parallel based on lane configuration.
    """
    print("🚀 Session Internal Parallel Processing Test\n")
    print("=" * 70)

    config_path = find_config_path()
    agent = Agent.create(config_path)

    # Test 1: Sequential tool execution (default, no queue)
    print("\n📦 Test 1: Sequential Tool Execution (No Queue)")
    print("-" * 70)
    print("Session without queue config - tools execute sequentially")
    print()

    session1 = agent.session(".")

    # This prompt will likely trigger multiple tool calls
    prompt1 = """
    Please do these tasks:
    1. Count how many .py files are in the current directory
    2. Count how many .rs files are in the current directory
    3. Count how many .js files are in the current directory

    Use the glob tool for each file type.
    """

    start = time.time()
    print("Sending prompt that triggers multiple tool calls...")
    result1 = session1.send(prompt1)
    duration1 = time.time() - start

    print(f"\n✓ Completed in {duration1:.2f}s")
    print(f"  Tool calls made: {result1.tool_calls_count}")
    print(f"  Response length: {len(result1.text)} chars")

    # Test 2: Parallel tool execution (with queue)
    print("\n\n⚡ Test 2: Parallel Tool Execution (With Queue)")
    print("-" * 70)
    print("Session with queue config - tools can execute in parallel")
    print()

    # Configure queue for parallel tool execution
    queue_config = SessionQueueConfig()
    queue_config.set_query_concurrency(3)      # Allow 3 concurrent query tools
    queue_config.set_execute_concurrency(2)    # Allow 2 concurrent execute tools
    queue_config.enable_metrics()

    session2 = agent.session(".", queue_config=queue_config)

    # Same prompt - should trigger same tool calls but execute in parallel
    prompt2 = """
    Please do these tasks:
    1. Count how many .py files are in the current directory
    2. Count how many .rs files are in the current directory
    3. Count how many .js files are in the current directory

    Use the glob tool for each file type.
    """

    start = time.time()
    print("Sending prompt that triggers multiple tool calls...")
    result2 = session2.send(prompt2)
    duration2 = time.time() - start

    print(f"\n✓ Completed in {duration2:.2f}s")
    print(f"  Tool calls made: {result2.tool_calls_count}")
    print(f"  Response length: {len(result2.text)} chars")

    # Check queue stats
    if session2.has_queue():
        stats = session2.queue_stats()
        print(f"\n📊 Queue Statistics:")
        print(f"  Total processed: {stats['total_processed']}")
        print(f"  Total failed: {stats['total_failed']}")

    # Compare performance
    print("\n\n📈 Performance Comparison")
    print("-" * 70)
    print(f"Sequential (no queue): {duration1:.2f}s")
    print(f"Parallel (with queue): {duration2:.2f}s")

    if duration1 > duration2:
        speedup = duration1 / duration2
        print(f"\n✓ Speedup: {speedup:.2f}x faster with parallel execution")
    else:
        print(f"\n⚠ No speedup observed (may need more tool calls to see benefit)")

    print("\n" + "=" * 70)


def test_complex_task_with_many_tools():
    """
    Test with a complex task that triggers many tool calls.

    This better demonstrates the internal parallel processing capability.
    """
    print("\n\n🔧 Test 3: Complex Task with Many Tool Calls")
    print("=" * 70)

    config_path = find_config_path()
    agent = Agent.create(config_path)

    # Configure for high concurrency
    queue_config = SessionQueueConfig()
    queue_config.set_query_concurrency(5)      # 5 concurrent query tools
    queue_config.set_execute_concurrency(3)    # 3 concurrent execute tools
    queue_config.enable_metrics()
    queue_config.enable_dlq()

    session = agent.session(".", queue_config=queue_config)

    # Complex prompt that should trigger many tool calls
    prompt = """
    Analyze this codebase:
    1. Find all Python files
    2. Find all Rust files
    3. Find all JavaScript files
    4. Find all TODO comments in Python files
    5. Find all TODO comments in Rust files
    6. Count total lines in all Python files

    Use appropriate tools (glob, grep) for each task.
    """

    print("Sending complex prompt...")
    print("Expected: Multiple tool calls executed in parallel by session queue")
    print()

    start = time.time()
    result = session.send(prompt)
    duration = time.time() - start

    print(f"\n✓ Completed in {duration:.2f}s")
    print(f"  Tool calls made: {result.tool_calls_count}")
    print(f"  Response length: {len(result.text)} chars")

    if session.has_queue():
        stats = session.queue_stats()
        print(f"\n📊 Queue Statistics:")
        print(f"  Total processed: {stats['total_processed']}")
        print(f"  Total failed: {stats['total_failed']}")
        print(f"  DLQ size: {stats['dlq_size']}")

    print("\n" + "=" * 70)


def main():
    """Run all tests."""
    print("\n" + "=" * 70)
    print("A3S Code Session - Internal Parallel Processing Test")
    print("=" * 70)
    print()
    print("This test demonstrates how A3S Code Session's internal queue")
    print("can execute multiple tool calls in parallel when triggered by")
    print("a single LLM response.")
    print()
    print("Key Concept:")
    print("  - ONE session.send() call")
    print("  - LLM decides to call MULTIPLE tools")
    print("  - Session queue executes tools IN PARALLEL")
    print("  - Based on lane configuration (Query/Execute/Generate)")
    print()

    try:
        test_internal_parallel_tool_execution()
        test_complex_task_with_many_tools()

        print("\n\n✅ All tests completed!")
        print("=" * 70)

    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()


if __name__ == "__main__":
    main()
