"""
Planning and Goal Tracking Example

Demonstrates how to use the planning and goal tracking system:
- Creating execution plans from prompts
- Extracting goals from natural language
- Checking goal achievement progress
- Combining planning with event streaming
"""

import asyncio
from a3s_code import A3sClient
from a3s_code.types import AgentGoal


async def planning_example():
    print("=" * 60)
    print("Planning and Goal Tracking Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="planning-demo",
            workspace="/tmp/planning-test",
            system_prompt="You are a helpful coding assistant that plans tasks carefully.",
            llm={
                "provider": "anthropic",
                "model": "claude-sonnet-4-20250514",
            },
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Create an Execution Plan
        # =====================================================================
        print("2. Creating execution plan...")
        plan_result = await client.create_plan(
            session_id=session_id,
            prompt="Create a REST API with user authentication using Python and Flask",
            context="The API should support JWT tokens and have endpoints for login, register, and profile.",
        )
        plan = plan_result.get("plan")
        if plan:
            print(f"✓ Execution plan:")
            print(f"  Goal: {plan.goal}")
            print(f"  Complexity: {plan.complexity.name if hasattr(plan.complexity, 'name') else plan.complexity}")
            print(f"  Estimated steps: {plan.estimated_steps}")
            for i, step in enumerate(plan.steps, 1):
                tool = step.tool if step.tool else "no-tool"
                print(f"  {i}. [{tool}] {step.description}")
        else:
            print("  (Plan data in raw format)")
            print(f"  {plan_result}")
        print()

        # =====================================================================
        # Get Plan by ID
        # =====================================================================
        print("3. Retrieving plan...")
        plan_id = plan_result.get("plan_id", "")
        if plan_id:
            retrieved_plan = await client.get_plan(
                session_id=session_id,
                plan_id=plan_id,
            )
            print(f"✓ Retrieved plan: {retrieved_plan.goal if hasattr(retrieved_plan, 'goal') else 'ok'}")
        else:
            print("  (No plan ID returned, skipping retrieval)")
        print()

        # =====================================================================
        # Extract Goal from Prompt
        # =====================================================================
        print("4. Extracting goal from natural language...")
        goal_result = await client.extract_goal(
            session_id=session_id,
            prompt="Fix all the bugs in the authentication module and add unit tests",
        )
        goal = goal_result.get("goal")
        if goal:
            print(f"✓ Extracted goal:")
            print(f"  Description: {goal.description}")
            if goal.success_criteria:
                print(f"  Success criteria:")
                for i, criterion in enumerate(goal.success_criteria, 1):
                    print(f"    {i}. {criterion}")
        else:
            print(f"  {goal_result}")
        print()

        # =====================================================================
        # Check Goal Achievement
        # =====================================================================
        print("5. Checking goal achievement...")
        check_result = await client.check_goal_achievement(
            session_id=session_id,
            goal=AgentGoal(
                description="Create a REST API",
                success_criteria=[
                    "API responds to HTTP requests",
                    "Authentication endpoints work",
                    "Unit tests pass",
                ],
                progress=0.5,
                achieved=False,
            ),
            current_state="API is running, authentication works, but tests are not written yet.",
        )
        print(f"✓ Goal achievement check:")
        print(f"  Achieved: {check_result.get('achieved', False)}")
        print(f"  Progress: {check_result.get('progress', 0) * 100:.1f}%")
        remaining = check_result.get("remaining_criteria", [])
        if remaining:
            print(f"  Remaining criteria:")
            for i, criterion in enumerate(remaining, 1):
                print(f"    {i}. {criterion}")
        print()

        # =====================================================================
        # Planning with Event Streaming
        # =====================================================================
        print("6. Generating with planning events...")

        # Subscribe to events in background
        async def handle_events():
            async for event in client.subscribe_events(session_id=session_id):
                event_type = event.get("type", "")
                if "PLANNING" in event_type or "PLAN" in event_type:
                    print(f"  [Plan Event] {event_type}: {event.get('data', {})}")
                elif event_type == "EVENT_TYPE_AGENT_END":
                    break

        event_task = asyncio.create_task(handle_events())

        # Generate with planning
        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "Create a simple hello world Flask server",
                }
            ],
        )
        if "message" in response:
            content = response["message"].get("content", "")
            print(f"✓ Response: {content[:200]}...")

        # Wait for events to finish
        try:
            await asyncio.wait_for(event_task, timeout=5.0)
        except asyncio.TimeoutError:
            event_task.cancel()
        print()

        # Clean up
        print("7. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Planning example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(planning_example())
