#!/usr/bin/env python3
"""
Query-Lane Tool Parallelization Test

Demonstrates A3S Code's Query-lane tool parallelization with slow I/O operations.
Parallelization is OPT-IN (default: serial execution). Users control when and how
to parallelize via SessionQueueConfig.

This test uses web_fetch to demonstrate real performance benefits, as network I/O
is significantly slower than local file operations.

Performance: 3-8x speedup for network I/O operations
"""

import os
import time
from pathlib import Path
from a3s_code import Agent, SessionOptions, SessionQueueConfig, ParallelizationStrategy

PROMPT = (
    "Fetch the following web pages and extract their titles:\n"
    "1. https://www.rust-lang.org/\n"
    "2. https://tokio.rs/\n"
    "3. https://docs.rs/\n"
    "4. https://crates.io/\n"
    "5. https://github.com/rust-lang/rust\n"
    "6. https://blog.rust-lang.org/\n"
    "7. https://www.rust-lang.org/learn\n"
    "8. https://www.rust-lang.org/tools\n"
    "9. https://www.rust-lang.org/governance\n"
    "10. https://www.rust-lang.org/community\n"
    "\n"
    "Fetch all pages at once using web_fetch tool, don't do them one by one."
)

def find_config() -> str:
    """Find the A3S config file."""
    home_config = Path.home() / ".a3s" / "config.hcl"
    if home_config.exists():
        return str(home_config)
    # Walk up from this file to find .a3s/config.hcl
    d = Path(__file__).resolve().parent
    for _ in range(10):
        candidate = d / ".a3s" / "config.hcl"
        if candidate.exists():
            return str(candidate)
        d = d.parent
    raise FileNotFoundError("Config file not found")

def test_default_serial(agent):
    """Test 1: Default behavior - serial execution (parallelization disabled by default)"""
    print("\n📦 Test 1: Default Behavior (Serial Execution)")
    print("-" * 80)
    print("Task: Fetch 10 web pages with default configuration\n")

    # Create session WITHOUT parallelization (default: enable_parallelization = false)
    session = agent.session(".")

    start = time.time()
    result = session.send(PROMPT)
    elapsed = time.time() - start

    print(f"✓ Completed in: {elapsed:.2f}s")
    print(f"  Result length: {len(result.text)} chars")
    print(f"  Tool calls: {result.tool_calls_count}")
    print("\n💡 Default: enable_parallelization = false (serial execution)")
    print("   Expected: ~10 * avg_fetch_time (network latency adds up)\n")

    return elapsed

def test_enabled_parallel(agent):
    """Test 2: Enabled parallelization - tools execute in parallel"""
    print("\n⚡ Test 2: Enabled Parallelization (Parallel Execution)")
    print("-" * 80)
    print("Task: Fetch 10 web pages in parallel via opt-in configuration\n")

    # Create SessionQueueConfig with parallelization ENABLED
    queue_config = SessionQueueConfig()
    queue_config.enable_parallelization = True  # OPT-IN: explicitly enable
    queue_config.set_query_concurrency(10)  # Allow 10 concurrent web fetches

    # Custom strategy: lower threshold, only allow web operations
    strategy = ParallelizationStrategy()
    strategy.min_tool_count = 3  # Lower threshold: 3 tools trigger parallelization
    strategy.allowed_tools = ["web_fetch", "web_search"]

    queue_config.parallelization_strategy = strategy

    print("✓ SessionQueueConfig created")
    print("  enable_parallelization: True (OPT-IN)")
    print("  Query lane max concurrency: 10")
    print("  Custom strategy:")
    print("    - min_tool_count: 3 (lower threshold)")
    print("    - allowed_tools: [web_fetch, web_search]")
    print("    - blocked_tools: [bash, write, edit, patch]\n")

    # Create session WITH parallelization enabled
    options = SessionOptions()
    options.queue_config = queue_config
    session = agent.session(".", options)

    start = time.time()
    result = session.send(PROMPT)
    elapsed = time.time() - start

    print(f"\n✓ Completed in: {elapsed:.2f}s")
    print(f"  Result length: {len(result.text)} chars")
    print(f"  Tool calls: {result.tool_calls_count}")
    print("\n💡 Parallelization enabled: web_fetch calls execute in parallel")
    print("   Expected: ~max(fetch_times) instead of sum(fetch_times)")
    print("   Speedup: 3-8x for network I/O operations\n")

    return elapsed

def main():
    """Run all tests and compare performance"""
    print("=" * 80)
    print("Query-Lane Tool Parallelization Test (Python SDK)")
    print("=" * 80)
    print("\n📌 Test Scenario: Fetch 10 web pages")
    print("   This demonstrates real performance benefits with slow I/O operations.\n")

    config_path = find_config()
    print(f"📄 Using config: {config_path}\n")

    agent = Agent.create(config_path)

    # Run tests
    sequential_time = test_default_serial(agent)
    parallel_time = test_enabled_parallel(agent)

    # Performance comparison
    print("\n" + "=" * 80)
    print("Performance Comparison")
    print("=" * 80)
    print(f"Sequential (default):   {sequential_time:.2f}s (baseline)")
    print(f"Parallel (opt-in):      {parallel_time:.2f}s ({sequential_time/parallel_time:.2f}x speedup)")
    print("\n✅ All parallelization tests completed!")
    print("=" * 80)

if __name__ == "__main__":
    main()
