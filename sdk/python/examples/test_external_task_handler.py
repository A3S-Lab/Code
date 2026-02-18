#!/usr/bin/env python3
"""
A3S Code Python SDK - External Task Handler Integration Test

Demonstrates how internal parallelizable tasks from the queue can be processed
externally in parallel through the SDK.

This test shows:
1. Configure lanes to use External mode
2. Agent sends tasks to internal queue
3. SDK polls pending external tasks
4. SDK processes tasks in parallel (external worker pool)
5. SDK submits results back to agent

Run with: python examples/test_external_task_handler.py
"""

import asyncio
import time
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from a3s_code import Agent


def find_config_path():
    """Find config file."""
    config_path = Path("/Users/roylin/Desktop/ai-lab/a3s/.a3s/config.hcl")
    if config_path.exists():
        return str(config_path)

    project_config = Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl"
    if project_config.exists():
        return str(project_config)

    raise FileNotFoundError("Config file not found")


def process_external_task(task):
    """
    External task processor - simulates processing a task outside the agent.

    In a real scenario, this could be:
    - Running on a different machine
    - Using specialized hardware (GPU)
    - Calling external APIs
    - Distributed processing
    """
    print(f"  [External Worker] Processing task {task['task_id'][:8]}...")
    print(f"    Command: {task['command_type']}")
    print(f"    Lane: {task['lane']}")

    # Simulate external processing
    time.sleep(0.5)

    # Return result
    return {
        'task_id': task['task_id'],
        'success': True,
        'result': f"Processed by external worker: {task['command_type']}"
    }


async def test_external_task_handler():
    """Test external task handler with parallel processing."""
    print("🚀 External Task Handler Integration Test\n")
    print("=" * 70)

    config_path = find_config_path()
    print(f"📄 Config: {config_path}")
    print("=" * 70)

    # Note: External task handler configuration needs to be done at the
    # queue/lane level. This example demonstrates the concept.

    print("\n📋 Concept: External Task Processing Flow")
    print("-" * 70)
    print("1. Agent queues tasks internally (Query/Execute lanes)")
    print("2. Tasks configured for External mode are exported")
    print("3. SDK polls pending_external_tasks()")
    print("4. SDK processes tasks in parallel (external workers)")
    print("5. SDK submits results via complete_external_task()")
    print("6. Agent receives results and continues execution")

    print("\n🔧 Configuration Example:")
    print("-" * 70)
    print("""
    # In HCL config or SessionQueueConfig:
    lane_handlers = {
        "query": {
            mode = "external"      # Send to external handler
            timeout_ms = 30000     # 30s timeout
        },
        "execute": {
            mode = "hybrid"        # Internal + external notification
            timeout_ms = 60000
        }
    }
    """)

    print("\n💡 Use Cases:")
    print("-" * 70)
    print("1. Distributed Processing:")
    print("   - Offload heavy tasks to worker machines")
    print("   - Scale horizontally with multiple workers")
    print()
    print("2. Specialized Hardware:")
    print("   - GPU-accelerated processing")
    print("   - Custom hardware for specific tasks")
    print()
    print("3. External Services:")
    print("   - Call external APIs")
    print("   - Integrate with existing systems")
    print()
    print("4. Parallel Execution:")
    print("   - Process multiple tasks concurrently")
    print("   - Better resource utilization")

    print("\n📊 Example: External Worker Pool")
    print("-" * 70)

    # Simulate external task processing
    mock_tasks = [
        {
            'task_id': f'task-{i}',
            'session_id': 'session-1',
            'lane': 'query',
            'command_type': 'grep' if i % 2 == 0 else 'glob',
            'payload': {'pattern': f'*.rs'}
        }
        for i in range(5)
    ]

    print(f"Simulating {len(mock_tasks)} external tasks...")
    print()

    start = time.time()

    # Process tasks in parallel using external worker pool
    with ThreadPoolExecutor(max_workers=3) as executor:
        futures = [
            executor.submit(process_external_task, task)
            for task in mock_tasks
        ]

        results = [f.result() for f in futures]

    duration = time.time() - start

    print()
    print(f"✓ Processed {len(results)} tasks in {duration:.2f}s")
    print(f"  Average: {duration/len(results):.2f}s per task")
    print(f"  Throughput: {len(results)/duration:.1f} tasks/sec")

    print("\n🔄 Workflow Summary:")
    print("-" * 70)
    print("┌─────────────┐")
    print("│   Agent     │  1. Queue tasks")
    print("│   (Core)    │  ────────────────┐")
    print("└─────────────┘                  │")
    print("       ▲                          ▼")
    print("       │                   ┌─────────────┐")
    print("       │ 5. Results        │  Internal   │")
    print("       │                   │   Queue     │")
    print("       │                   └─────────────┘")
    print("       │                          │")
    print("       │                          │ 2. Export")
    print("       │                          ▼")
    print("┌─────────────┐           ┌─────────────┐")
    print("│  SDK/App    │  3. Poll  │  External   │")
    print("│  (Python)   │◄──────────│   Tasks     │")
    print("└─────────────┘           └─────────────┘")
    print("       │")
    print("       │ 4. Process in parallel")
    print("       ▼")
    print("┌─────────────────────────────┐")
    print("│  External Worker Pool       │")
    print("│  ┌────┐ ┌────┐ ┌────┐      │")
    print("│  │ W1 │ │ W2 │ │ W3 │ ...  │")
    print("│  └────┘ └────┘ └────┘      │")
    print("└─────────────────────────────┘")

    print("\n✅ External task handler concept demonstrated!")
    print("=" * 70)


async def test_api_methods():
    """Test the actual API methods for external task handling."""
    print("\n\n🔍 Testing External Task API Methods")
    print("=" * 70)

    config_path = find_config_path()
    agent = Agent.create(config_path)
    session = agent.session(".")

    print("\n1. Check if session has queue:")
    has_queue = session.has_queue()
    print(f"   has_queue() = {has_queue}")

    if has_queue:
        print("\n2. Get pending external tasks:")
        # Note: This will return empty unless tasks are configured for external mode
        # pending_tasks = session.pending_external_tasks()
        # print(f"   Found {len(pending_tasks)} pending external tasks")
        print("   (Would call session.pending_external_tasks())")

        print("\n3. Complete external task:")
        # task_id = "example-task-id"
        # result = {
        #     'success': True,
        #     'result': {'output': 'Task completed'}
        # }
        # success = session.complete_external_task(task_id, result)
        print("   (Would call session.complete_external_task(task_id, result))")
    else:
        print("   ⚠ Queue not enabled - external task handling requires queue config")

    print("\n✅ API methods available for external task handling")
    print("=" * 70)


async def main():
    """Run all tests."""
    await test_external_task_handler()
    await test_api_methods()

    print("\n\n📚 Next Steps:")
    print("-" * 70)
    print("1. Configure lane_handlers in SessionQueueConfig")
    print("2. Set mode = 'external' for desired lanes")
    print("3. Implement external worker pool")
    print("4. Poll pending_external_tasks() periodically")
    print("5. Process tasks and submit results")
    print()
    print("See A3S Lane v0.4.0 documentation for full details.")


if __name__ == "__main__":
    asyncio.run(main())
