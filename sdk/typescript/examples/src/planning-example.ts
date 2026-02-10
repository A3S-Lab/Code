/**
 * Planning and Goal Tracking Example
 *
 * This example demonstrates how to use the planning and goal tracking
 * features of the A3S Code Agent.
 */

import { CodeAgentClient } from '../../src/index.js';

async function main() {
  // Connect to the agent
  const client = new CodeAgentClient('localhost:50051');

  try {
    // Initialize the agent
    console.log('Initializing agent...');
    await client.initialize({ workspace: '/tmp/planning-demo' });

    // Create a session
    console.log('Creating session...');
    const session = await client.createSession({
      name: 'planning-demo',
      systemPrompt: 'You are a helpful coding assistant that plans tasks carefully.',
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}`);

    // Example 1: Create an execution plan
    console.log('\n=== Example 1: Create Execution Plan ===');
    try {
      const planResponse = await client.createPlan({
        sessionId,
        prompt: 'Create a REST API with user authentication using Node.js and Express',
        context: 'The API should support JWT tokens and have endpoints for login, register, and profile.',
      });
      console.log('Execution Plan:');
      console.log(`  Goal: ${planResponse.plan?.goal}`);
      console.log(`  Complexity: ${planResponse.plan?.complexity}`);
      console.log(`  Steps: ${planResponse.plan?.estimatedSteps}`);
      planResponse.plan?.steps?.forEach((step, i) => {
        console.log(`    ${i + 1}. [${step.tool || 'no-tool'}] ${step.description}`);
      });
    } catch (error: any) {
      if (error.code === 12) {
        // UNIMPLEMENTED
        console.log('  (Planning RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 2: Extract goal from prompt
    console.log('\n=== Example 2: Extract Goal ===');
    try {
      const goalResponse = await client.extractGoal({
        sessionId,
        prompt: 'Fix all the bugs in the authentication module and add unit tests',
      });
      console.log('Extracted Goal:');
      console.log(`  Description: ${goalResponse.goal?.description}`);
      console.log(`  Success Criteria:`);
      goalResponse.goal?.successCriteria?.forEach((criterion, i) => {
        console.log(`    ${i + 1}. ${criterion}`);
      });
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Goal extraction RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 3: Check goal achievement
    console.log('\n=== Example 3: Check Goal Achievement ===');
    try {
      const checkResponse = await client.checkGoalAchievement({
        sessionId,
        goal: {
          description: 'Create a REST API',
          successCriteria: [
            'API responds to HTTP requests',
            'Authentication endpoints work',
            'Unit tests pass',
          ],
          progress: 0.5,
          achieved: false,
        },
        currentState: 'API is running, authentication works, but tests are not written yet.',
      });
      console.log('Goal Achievement Check:');
      console.log(`  Achieved: ${checkResponse.achieved}`);
      console.log(`  Progress: ${(checkResponse.progress * 100).toFixed(1)}%`);
      if (checkResponse.remainingCriteria?.length) {
        console.log(`  Remaining Criteria:`);
        checkResponse.remainingCriteria.forEach((criterion, i) => {
          console.log(`    ${i + 1}. ${criterion}`);
        });
      }
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Goal achievement RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 4: Generate with planning events
    console.log('\n=== Example 4: Generate with Planning Events ===');
    console.log('Subscribing to events...');

    // Subscribe to events
    const eventStream = client.subscribeEvents({ sessionId });

    // Handle events in background
    const eventPromise = (async () => {
      for await (const event of eventStream) {
        const eventType = event.eventType;
        switch (eventType) {
          case 'planning_start':
            console.log(`  [Event] Planning started: ${event.data?.prompt}`);
            break;
          case 'planning_end':
            console.log(`  [Event] Planning completed: ${event.data?.estimatedSteps} steps`);
            break;
          case 'step_start':
            console.log(
              `  [Event] Step ${event.data?.stepNumber}/${event.data?.totalSteps}: ${event.data?.description}`
            );
            break;
          case 'step_end':
            console.log(`  [Event] Step ${event.data?.stepId} completed: ${event.data?.status}`);
            break;
          case 'goal_progress':
            console.log(
              `  [Event] Goal progress: ${(event.data?.progress * 100).toFixed(1)}%`
            );
            break;
          case 'goal_achieved':
            console.log(`  [Event] Goal achieved: ${event.data?.goal}`);
            break;
          case 'text_delta':
            process.stdout.write(event.data?.text || '');
            break;
          case 'agent_end':
            console.log('\n  [Event] Agent completed');
            break;
        }
      }
    })();

    // Generate response
    console.log('Generating response...');
    const response = await client.generate({
      sessionId,
      prompt: 'Create a simple hello world Express server',
    });
    console.log(`\nFinal response length: ${response.text?.length} characters`);

    // Clean up
    console.log('\n=== Cleanup ===');
    await client.destroySession({ sessionId });
    console.log('Session destroyed');

    await client.shutdown({});
    console.log('Agent shutdown complete');
  } catch (error) {
    console.error('Error:', error);
    process.exit(1);
  }
}

main();
