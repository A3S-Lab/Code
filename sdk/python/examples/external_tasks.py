"""
External Task Handling Example

Demonstrates how to delegate task execution to external systems:
- Configure lane handlers (Internal/External/Hybrid modes)
- Poll and process external tasks
- Complete external tasks with results
- Use case: Secure sandbox execution
"""

import asyncio
import json
from a3s_code import A3sClient, SessionLane, TaskHandlerMode


class ExternalTaskExecutor:
    """Simulated external system for task execution"""

    async def execute(self, command_type: str, payload: dict) -> dict:
        print(f"  [External System] Executing {command_type}...")

        # Simulate execution delay
        await asyncio.sleep(1)

        # Simulate different command types
        if command_type == "Bash":
            return {
                "stdout": f"Executed: {payload.get('command')}\nOutput from external system",
                "stderr": "",
                "exitCode": 0
            }

        return {"result": "Task completed by external system"}


async def external_task_example():
    print("=" * 60)
    print("External Task Handling Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        executor = ExternalTaskExecutor()

        try:
            # Create a session
            print("1. Creating session...")
            session = await client.create_session(
                name="External Task Demo",
                workspace="/tmp/workspace",
                system_prompt="You are an assistant that executes tasks in a secure external environment."
            )
            session_id = session['session_id']
            print(f"✓ Session created: {session_id}")
            print()

            # Configure Execute lane for external handling
            print("2. Configuring Execute lane for external handling...")
            await client.set_lane_handler(
                session_id,
                SessionLane.SESSION_LANE_EXECUTE,
                mode=TaskHandlerMode.TASK_HANDLER_MODE_EXTERNAL,
                timeout_ms=60000
            )
            print("✓ Execute lane configured:")
            print("  Mode: External")
            print("  Timeout: 60s")
            print()

            # Start external task processor
            print("3. Starting external task processor...")
            processor_running = True

            async def process_external_tasks():
                """Process external tasks in background"""
                while processor_running:
                    try:
                        # Poll for pending tasks
                        tasks = await client.list_pending_external_tasks(session_id)

                        for task in tasks.get('tasks', []):
                            print(f"\n📋 External task received:")
                            print(f"  Task ID: {task['taskId']}")
                            print(f"  Lane: {task['lane']}")
                            print(f"  Command: {task['commandType']}")
                            print(f"  Timeout: {task['timeoutMs']}ms")
                            print(f"  Remaining: {task['remainingMs']}ms")

                            try:
                                # Parse payload
                                payload = json.loads(task['payload'])
                                print(f"  Payload: {json.dumps(payload, indent=2)}")

                                # Execute in external system
                                result = await executor.execute(task['commandType'], payload)

                                # Complete the task
                                await client.complete_external_task(
                                    session_id,
                                    task['taskId'],
                                    success=True,
                                    result=json.dumps(result)
                                )

                                print("  ✓ Task completed successfully")

                            except Exception as error:
                                # Report failure
                                await client.complete_external_task(
                                    session_id,
                                    task['taskId'],
                                    success=False,
                                    error=str(error)
                                )

                                print(f"  ✗ Task failed: {error}")

                        # Poll every 500ms
                        await asyncio.sleep(0.5)

                    except Exception as error:
                        if processor_running:
                            print(f"Processor error: {error}")

            # Start processor in background
            processor_task = asyncio.create_task(process_external_tasks())
            print("✓ External task processor started")
            print()

            # Example 4: Trigger external task execution
            print("4. Triggering external task execution...")
            print("  Sending request to execute bash command...")

            response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": 'Run the command: echo "Hello from external system"'
                }]
            )

            print(f"\n✓ Response received:")
            print(f"  {response['content'][:200]}...")
            print()

            # Example 5: Check lane handler configuration
            print("5. Checking lane handler configuration...")
            handler_config = await client.get_lane_handler(
                session_id,
                SessionLane.SESSION_LANE_EXECUTE
            )
            print("✓ Current configuration:")
            print(f"  Mode: {handler_config.get('mode')}")
            print(f"  Timeout: {handler_config.get('timeoutMs')}ms")
            print()

            # Example 6: Hybrid mode demonstration
            print("6. Configuring Hybrid mode for Generate lane...")
            await client.set_lane_handler(
                session_id,
                SessionLane.SESSION_LANE_GENERATE,
                mode=TaskHandlerMode.TASK_HANDLER_MODE_HYBRID,
                timeout_ms=120000
            )
            print("✓ Generate lane configured:")
            print("  Mode: Hybrid (internal execution + external notification)")
            print("  Use case: Monitor LLM calls while executing internally")
            print()

            # Wait a bit for any pending tasks
            await asyncio.sleep(2)

            # Cleanup
            print("7. Cleanup...")
            processor_running = False
            processor_task.cancel()
            try:
                await processor_task
            except asyncio.CancelledError:
                pass

            await client.destroy_session(session_id)
            print("✓ Session destroyed")
            print()

            print("=" * 60)
            print("External Task Handling Example Complete")
            print("=" * 60)
            print()
            print("Use cases:")
            print("  - Execute commands in secure sandboxes")
            print("  - Delegate tasks to specialized systems")
            print("  - Monitor and audit tool executions")
            print("  - Implement custom execution policies")

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(external_task_example())
