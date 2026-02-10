"""
Planning and Goal Tracking Example

This example demonstrates how to use the planning and goal tracking
features of the A3S Code Agent.
"""

import asyncio
from a3s_code import CodeAgentClient


async def main():
    # Connect to the agent
    client = CodeAgentClient('localhost:50051')

    try:
        # Initialize the agent
        print('Initializing agent...')
        await client.initialize(workspace='/tmp/planning-demo')

        # Create a session
        print('Creating session...')
        session = await client.create_session(
            name='planning-demo',
            system_prompt='You are a helpful coding assistant that plans tasks carefully.'
        )
        session_id = session.session_id
        print(f'Session created: {session_id}')

        # Example 1: Create an execution plan
        print('\n=== Example 1: Create Execution Plan ===')
        try:
            plan_response = await client.create_plan(
                session_id=session_id,
                prompt='Create a REST API with user authentication using Python and Flask',
                context='The API should support JWT tokens and have endpoints for login, register, and profile.'
            )
            print('Execution Plan:')
            print(f'  Goal: {plan_response.plan.goal}')
            print(f'  Complexity: {plan_response.plan.complexity}')
            print(f'  Steps: {plan_response.plan.estimated_steps}')
            for i, step in enumerate(plan_response.plan.steps, 1):
                tool = step.tool if step.tool else 'no-tool'
                print(f'    {i}. [{tool}] {step.description}')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Planning RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 2: Extract goal from prompt
        print('\n=== Example 2: Extract Goal ===')
        try:
            goal_response = await client.extract_goal(
                session_id=session_id,
                prompt='Fix all the bugs in the authentication module and add unit tests'
            )
            print('Extracted Goal:')
            print(f'  Description: {goal_response.goal.description}')
            print('  Success Criteria:')
            for i, criterion in enumerate(goal_response.goal.success_criteria, 1):
                print(f'    {i}. {criterion}')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Goal extraction RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 3: Check goal achievement
        print('\n=== Example 3: Check Goal Achievement ===')
        try:
            check_response = await client.check_goal_achievement(
                session_id=session_id,
                goal={
                    'description': 'Create a REST API',
                    'success_criteria': [
                        'API responds to HTTP requests',
                        'Authentication endpoints work',
                        'Unit tests pass'
                    ],
                    'progress': 0.5,
                    'achieved': False
                },
                current_state='API is running, authentication works, but tests are not written yet.'
            )
            print('Goal Achievement Check:')
            print(f'  Achieved: {check_response.achieved}')
            print(f'  Progress: {check_response.progress * 100:.1f}%')
            if check_response.remaining_criteria:
                print('  Remaining Criteria:')
                for i, criterion in enumerate(check_response.remaining_criteria, 1):
                    print(f'    {i}. {criterion}')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Goal achievement RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 4: Generate with planning events
        print('\n=== Example 4: Generate with Planning Events ===')
        print('Subscribing to events...')

        # Subscribe to events
        event_stream = client.subscribe_events(session_id=session_id)

        # Handle events in background
        async def handle_events():
            async for event in event_stream:
                event_type = event.event_type
                if event_type == 'planning_start':
                    print(f'  [Event] Planning started: {event.data.get("prompt")}')
                elif event_type == 'planning_end':
                    print(f'  [Event] Planning completed: {event.data.get("estimatedSteps")} steps')
                elif event_type == 'step_start':
                    print(f'  [Event] Step {event.data.get("stepNumber")}/{event.data.get("totalSteps")}: {event.data.get("description")}')
                elif event_type == 'step_end':
                    print(f'  [Event] Step {event.data.get("stepId")} completed: {event.data.get("status")}')
                elif event_type == 'goal_progress':
                    print(f'  [Event] Goal progress: {event.data.get("progress") * 100:.1f}%')
                elif event_type == 'goal_achieved':
                    print(f'  [Event] Goal achieved: {event.data.get("goal")}')
                elif event_type == 'text_delta':
                    print(event.data.get('text', ''), end='', flush=True)
                elif event_type == 'agent_end':
                    print('\n  [Event] Agent completed')
                    break

        # Start event handler
        event_task = asyncio.create_task(handle_events())

        # Generate response
        print('Generating response...')
        response = await client.generate(
            session_id=session_id,
            prompt='Create a simple hello world Flask server'
        )
        print(f'\nFinal response length: {len(response.text)} characters')

        # Wait for events to finish
        await event_task

        # Clean up
        print('\n=== Cleanup ===')
        await client.destroy_session(session_id=session_id)
        print('Session destroyed')

        await client.shutdown()
        print('Agent shutdown complete')

    except Exception as error:
        print(f'Error: {error}')
        raise


if __name__ == '__main__':
    asyncio.run(main())
