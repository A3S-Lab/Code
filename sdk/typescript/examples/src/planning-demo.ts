/**
 * Planning & Goal Tracking Demo
 *
 * Demonstrates how to use the Planning & Goal Tracking APIs:
 * - createPlan() - Create execution plans
 * - getPlan() - Get existing plans
 * - extractGoal() - Extract goals from prompts
 * - checkGoalAchievement() - Check goal completion
 */

import { A3sClient, StorageType } from '@a3s-lab/code';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    // Create a session
    console.log('=== Creating Session ===\n');
    const session = await client.createSession({
      name: 'planning-demo',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}\n`);

    // Example 1: Extract Goal
    console.log('=== Example 1: Extract Goal ===\n');
    const goalPrompt = 'Build a REST API for a todo application with user authentication';

    const goalResponse = await client.extractGoal(sessionId, goalPrompt);
    if (goalResponse.goal) {
      console.log('Extracted Goal:');
      console.log(`  Description: ${goalResponse.goal.description}`);
      console.log(`  Success Criteria:`);
      goalResponse.goal.successCriteria.forEach((criterion, i) => {
        console.log(`    ${i + 1}. ${criterion}`);
      });
      console.log(`  Progress: ${goalResponse.goal.progress * 100}%`);
      console.log(`  Achieved: ${goalResponse.goal.achieved}`);
    }
    console.log('\n');

    // Example 2: Create Plan
    console.log('=== Example 2: Create Plan ===\n');
    const planPrompt = 'Implement user authentication with JWT tokens';
    const context = 'Using Node.js, Express, and PostgreSQL';

    const planResponse = await client.createPlan(sessionId, planPrompt, context);
    if (planResponse.plan) {
      const plan = planResponse.plan;
      console.log('Execution Plan:');
      console.log(`  Goal: ${plan.goal}`);
      console.log(`  Complexity: ${plan.complexity}`);
      console.log(`  Estimated Steps: ${plan.estimatedSteps}`);
      console.log(`  Required Tools: ${plan.requiredTools.join(', ')}`);
      console.log('\n  Steps:');
      plan.steps.forEach((step, i) => {
        console.log(`    ${i + 1}. ${step.description}`);
        console.log(`       Status: ${step.status}`);
        if (step.tool) {
          console.log(`       Tool: ${step.tool}`);
        }
        if (step.dependencies.length > 0) {
          console.log(`       Dependencies: ${step.dependencies.join(', ')}`);
        }
      });
    }
    console.log('\n');

    // Example 3: Check Goal Achievement
    console.log('=== Example 3: Check Goal Achievement ===\n');

    if (goalResponse.goal) {
      const currentState = `
        - Created Express server
        - Set up PostgreSQL database
        - Implemented user registration endpoint
        - Implemented login endpoint with JWT
        - Added authentication middleware
      `;

      const achievementResponse = await client.checkGoalAchievement(
        sessionId,
        goalResponse.goal,
        currentState
      );

      console.log('Goal Achievement Check:');
      console.log(`  Achieved: ${achievementResponse.achieved}`);
      console.log(`  Progress: ${achievementResponse.progress * 100}%`);
      if (achievementResponse.remainingCriteria.length > 0) {
        console.log('  Remaining Criteria:');
        achievementResponse.remainingCriteria.forEach((criterion, i) => {
          console.log(`    ${i + 1}. ${criterion}`);
        });
      } else {
        console.log('  All criteria met! 🎉');
      }
    }
    console.log('\n');

    // Example 4: Get Plan (if we had a plan ID)
    console.log('=== Example 4: Get Plan ===\n');
    console.log('Note: getPlan() requires a plan ID from a previously created plan.');
    console.log('In a real application, you would store the plan ID and retrieve it later.\n');

    // Cleanup
    console.log('=== Cleanup ===\n');
    await client.destroySession(sessionId);
    console.log('Session destroyed');

  } catch (error) {
    console.error('Error:', error);
  } finally {
    client.close();
  }
}

main();
