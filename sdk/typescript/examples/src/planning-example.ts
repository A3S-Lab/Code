/**
 * Planning and Goal Tracking Example
 *
 * Demonstrates how to use the planning and goal tracking system:
 * - Creating execution plans from prompts
 * - Extracting goals from natural language
 * - Checking goal achievement progress
 */

import { A3sClient } from '@a3s-lab/code';

async function planningExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Planning and Goal Tracking Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'planning-demo',
      workspace: '/tmp/planning-test',
      systemPrompt: 'You are a helpful coding assistant that plans tasks carefully.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Create an execution plan
    console.log('2. Creating execution plan...');
    const planResult = await client.createPlan(
      sessionId,
      'Create a REST API with user authentication using Python and Flask',
      'The API should support JWT tokens and have endpoints for login, register, and profile.',
    );
    if (planResult.plan) {
      const plan = planResult.plan;
      console.log('✓ Execution plan:');
      console.log(`  Goal: ${plan.goal}`);
      console.log(`  Complexity: ${plan.complexity}`);
      console.log(`  Estimated steps: ${plan.estimatedSteps}`);
      for (const [i, step] of (plan.steps || []).entries()) {
        const tool = step.tool || 'no-tool';
        console.log(`  ${i + 1}. [${tool}] ${step.description}`);
      }
    } else {
      console.log(`  ${JSON.stringify(planResult)}`);
    }
    console.log();

    // Get plan by ID
    console.log('3. Retrieving plan...');
    const planId = planResult.planId || '';
    if (planId) {
      const retrieved = await client.getPlan(sessionId, planId);
      console.log(`✓ Retrieved plan: ${retrieved.plan?.goal || 'ok'}`);
    } else {
      console.log('  (No plan ID returned, skipping retrieval)');
    }
    console.log();

    // Extract goal from prompt
    console.log('4. Extracting goal from natural language...');
    const goalResult = await client.extractGoal(
      sessionId,
      'Fix all the bugs in the authentication module and add unit tests',
    );
    if (goalResult.goal) {
      console.log('✓ Extracted goal:');
      console.log(`  Description: ${goalResult.goal.description}`);
      if (goalResult.goal.successCriteria?.length) {
        console.log('  Success criteria:');
        for (const [i, criterion] of goalResult.goal.successCriteria.entries()) {
          console.log(`    ${i + 1}. ${criterion}`);
        }
      }
    } else {
      console.log(`  ${JSON.stringify(goalResult)}`);
    }
    console.log();

    // Check goal achievement
    console.log('5. Checking goal achievement...');
    const checkResult = await client.checkGoalAchievement(
      sessionId,
      {
        description: 'Create a REST API',
        successCriteria: [
          'API responds to HTTP requests',
          'Authentication endpoints work',
          'Unit tests pass',
        ],
        progress: 0.5,
        achieved: false,
      },
      'API is running, authentication works, but tests are not written yet.',
    );
    console.log('✓ Goal achievement check:');
    console.log(`  Achieved: ${checkResult.achieved}`);
    console.log(`  Progress: ${((checkResult.progress || 0) * 100).toFixed(1)}%`);
    if (checkResult.remainingCriteria?.length) {
      console.log('  Remaining criteria:');
      for (const [i, criterion] of checkResult.remainingCriteria.entries()) {
        console.log(`    ${i + 1}. ${criterion}`);
      }
    }
    console.log();

    // Clean up
    console.log('6. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Planning example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

planningExample();
