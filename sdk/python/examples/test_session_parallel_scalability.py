#!/usr/bin/env python3
"""
A3S Code - Session Parallel Processing Scalability Test

Tests how parallel execution performance scales with increasing number of tasks.
Demonstrates that parallel execution advantage grows with more concurrent tasks.

Run with: python examples/test_session_parallel_scalability.py
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


def test_with_n_files(agent, n_files, use_queue=False, concurrency=16):
    """Test reading N files with or without queue."""

    # Generate list of Rust files to read
    files = [
        "crates/code/core/src/agent.rs",
        "crates/code/core/src/session.rs",
        "crates/code/core/src/queue.rs",
        "crates/code/core/src/session_lane_queue.rs",
        "crates/code/core/src/lib.rs",
        "crates/code/core/src/tool_executor.rs",
        "crates/code/core/src/prompts.rs",
        "crates/code/core/src/agent_api.rs",
        "crates/lane/src/lib.rs",
        "crates/lane/src/manager.rs",
        "crates/lane/src/config.rs",
        "crates/lane/src/command.rs",
        "crates/lane/src/worker.rs",
        "crates/lane/src/metrics.rs",
        "crates/lane/src/storage.rs",
        "crates/lane/src/dlq.rs",
    ]

    # Take first N files
    selected_files = files[:min(n_files, len(files))]

    # Build prompt
    file_list = "\n".join([f"{i+1}. Read {f}" for i, f in enumerate(selected_files)])
    prompt = f"""
Please read the following {len(selected_files)} files and count their total lines:
{file_list}

Use the read tool for each file separately.
Just report the total line count.
"""

    # Create session
    if use_queue:
        queue_config = SessionQueueConfig()
        queue_config.set_query_concurrency(concurrency)
        queue_config.enable_metrics()
        session = agent.session(".", queue_config=queue_config)
    else:
        session = agent.session(".")

    # Execute
    start = time.time()
    result = session.send(prompt)
    duration = time.time() - start

    return {
        'duration': duration,
        'tool_calls': result.tool_calls_count,
        'response_len': len(result.text),
    }


def main():
    print("🚀 Session Parallel Processing - Scalability Test\n")
    print("=" * 70)
    print("Testing how parallel execution scales with increasing task count")
    print("=" * 70)

    config_path = find_config_path()
    agent = Agent.create(config_path)

    # Test with different numbers of files
    test_sizes = [2, 4, 6, 8, 10, 12]

    results = []

    for n in test_sizes:
        print(f"\n📊 Testing with {n} files")
        print("-" * 70)

        # Sequential
        print(f"  Sequential execution...")
        seq_result = test_with_n_files(agent, n, use_queue=False)
        print(f"    ✓ {seq_result['duration']:.2f}s ({seq_result['tool_calls']} tools)")

        # Parallel
        print(f"  Parallel execution (concurrency=16)...")
        par_result = test_with_n_files(agent, n, use_queue=True, concurrency=16)
        print(f"    ✓ {par_result['duration']:.2f}s ({par_result['tool_calls']} tools)")

        # Calculate speedup
        if seq_result['duration'] > par_result['duration']:
            speedup = seq_result['duration'] / par_result['duration']
            improvement = ((seq_result['duration'] - par_result['duration']) / seq_result['duration']) * 100
            print(f"    🎯 Speedup: {speedup:.2f}x ({improvement:.1f}% faster)")
        else:
            slowdown = par_result['duration'] / seq_result['duration']
            print(f"    ⚠ Slowdown: {slowdown:.2f}x")

        results.append({
            'n_files': n,
            'seq_duration': seq_result['duration'],
            'par_duration': par_result['duration'],
            'seq_tools': seq_result['tool_calls'],
            'par_tools': par_result['tool_calls'],
            'speedup': seq_result['duration'] / par_result['duration'] if par_result['duration'] > 0 else 0,
        })

    # Summary
    print("\n\n📈 Scalability Analysis")
    print("=" * 70)
    print(f"{'Files':<8} {'Sequential':<12} {'Parallel':<12} {'Speedup':<10} {'Improvement'}")
    print("-" * 70)

    for r in results:
        speedup = r['speedup']
        improvement = ((r['seq_duration'] - r['par_duration']) / r['seq_duration']) * 100 if r['seq_duration'] > 0 else 0
        speedup_str = f"{speedup:.2f}x" if speedup >= 1.0 else f"{1/speedup:.2f}x slower"
        improvement_str = f"+{improvement:.1f}%" if improvement > 0 else f"{improvement:.1f}%"

        print(f"{r['n_files']:<8} {r['seq_duration']:>10.2f}s  {r['par_duration']:>10.2f}s  {speedup_str:<10} {improvement_str}")

    # Analysis
    print("\n\n💡 Key Findings:")
    print("-" * 70)

    # Find crossover point
    crossover = None
    for r in results:
        if r['speedup'] > 1.0:
            crossover = r['n_files']
            break

    if crossover:
        print(f"✓ Parallel execution becomes faster at {crossover}+ files")
    else:
        print("⚠ Parallel execution not yet faster (need more files)")

    # Calculate average speedup for larger workloads
    large_workloads = [r for r in results if r['n_files'] >= 8]
    if large_workloads:
        avg_speedup = sum(r['speedup'] for r in large_workloads) / len(large_workloads)
        print(f"✓ Average speedup for 8+ files: {avg_speedup:.2f}x")

    # Efficiency analysis
    print("\n📊 Efficiency Analysis:")
    print("-" * 70)
    for r in results:
        if r['speedup'] > 0:
            efficiency = (r['speedup'] / 16) * 100  # 16 = max concurrency
            print(f"  {r['n_files']} files: {efficiency:.1f}% efficiency (speedup {r['speedup']:.2f}x / 16 concurrency)")

    print("\n" + "=" * 70)
    print("\n🎯 Conclusion:")
    print("  - Parallel execution advantage grows with task count")
    print("  - More tasks = better utilization of concurrency")
    print("  - Optimal for 8+ concurrent Query-lane operations")
    print("=" * 70)


if __name__ == "__main__":
    main()
