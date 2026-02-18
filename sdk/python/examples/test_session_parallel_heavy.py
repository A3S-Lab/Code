#!/usr/bin/env python3
"""
A3S Code - Session Internal Parallel Processing Test (Heavy Operations)

Tests Session's internal parallel execution with heavier Query-lane operations.

Query-lane tools (read, grep) can execute in parallel when:
1. Session has queue configured
2. LLM calls multiple Query-lane tools in one response
3. Queue executes them concurrently based on query_max_concurrency

This test uses heavier operations (grep with large codebases) to demonstrate
the actual benefit of parallel execution.

Run with: python examples/test_session_parallel_heavy.py
"""

import time
from pathlib import Path
from a3s_code import Agent, SessionQueueConfig


def find_config_path():
    """Find config file."""
    config_path = Path("/Users/roylin/Desktop/ai-lab/a3s/.a3s/config.hcl")
    if config_path.exists():
        return str(config_path)
    raise FileNotFoundError("Config file not found")


def main():
    print("🚀 Session Internal Parallel Processing - Heavy Operations Test\n")
    print("=" * 70)

    config_path = find_config_path()
    agent = Agent.create(config_path)

    # Test prompt that triggers multiple heavy Query-lane tool calls
    # Using grep to search through large Rust codebase
    prompt = """
    Please search the codebase for the following patterns using grep:
    1. Search for "async fn" in all Rust files
    2. Search for "pub struct" in all Rust files
    3. Search for "impl " in all Rust files
    4. Search for "use " in all Rust files
    5. Search for "mod " in all Rust files

    Use grep tool separately for each pattern.
    Just report the count of matches for each pattern.
    """

    # Test 1: Without queue (sequential)
    print("\n📦 Test 1: Without Queue (Sequential Execution)")
    print("-" * 70)

    session1 = agent.session(".")

    start1 = time.time()
    result1 = session1.send(prompt)
    duration1 = time.time() - start1

    print(f"✓ Completed in {duration1:.2f}s")
    print(f"  Tool calls: {result1.tool_calls_count}")
    print(f"  Response: {len(result1.text)} chars")

    # Test 2: With queue (parallel)
    print("\n⚡ Test 2: With Queue (Parallel Execution)")
    print("-" * 70)
    print("Queue config: query_concurrency=5 (allows 5 concurrent Query tools)")
    print()

    queue_config = SessionQueueConfig()
    queue_config.set_query_concurrency(5)  # Allow 5 concurrent Query-lane tools
    queue_config.enable_metrics()

    session2 = agent.session(".", queue_config=queue_config)

    start2 = time.time()
    result2 = session2.send(prompt)
    duration2 = time.time() - start2

    print(f"✓ Completed in {duration2:.2f}s")
    print(f"  Tool calls: {result2.tool_calls_count}")
    print(f"  Response: {len(result2.text)} chars")

    if session2.has_queue():
        stats = session2.queue_stats()
        print(f"\n📊 Queue Stats:")
        print(f"  Processed: {stats['total_processed']}")
        print(f"  Failed: {stats['total_failed']}")

    # Compare
    print("\n📈 Performance Comparison")
    print("-" * 70)
    print(f"Sequential: {duration1:.2f}s ({result1.tool_calls_count} tools)")
    print(f"Parallel:   {duration2:.2f}s ({result2.tool_calls_count} tools)")

    if duration1 > duration2:
        speedup = duration1 / duration2
        improvement = ((duration1 - duration2) / duration1) * 100
        print(f"\n✓ Speedup: {speedup:.2f}x ({improvement:.1f}% faster)")
    elif duration2 > duration1:
        slowdown = duration2 / duration1
        print(f"\n⚠ Slowdown: {slowdown:.2f}x (parallel is slower)")
        print("  Possible reasons:")
        print("  - Queue overhead exceeds benefit for these operations")
        print("  - Different number of tool calls")
        print("  - LLM made different decisions")
    else:
        print("\n≈ Similar performance")

    print("\n" + "=" * 70)
    print("\n💡 Key Points:")
    print("  - Query-lane tools: read, glob, grep, ls (can run in parallel)")
    print("  - Execute-lane tools: bash, write, edit (run sequentially)")
    print("  - Parallel execution happens INSIDE one session.send() call")
    print("  - When LLM calls multiple Query tools, they execute concurrently")
    print("  - Benefit is most visible with heavier operations (grep, read large files)")
    print("=" * 70)


if __name__ == "__main__":
    main()
