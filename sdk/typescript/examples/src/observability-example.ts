/**
 * Observability Example
 *
 * Demonstrates how to monitor agent performance and costs:
 * - Tool usage metrics (call counts, durations, success/failure rates)
 * - LLM cost tracking (per-model, per-day breakdowns)
 * - Using metrics for optimization decisions
 */

import { A3sClient } from '@a3s-lab/code';

async function observabilityExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Observability Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session and generate some activity
    console.log('1. Creating session and generating activity...');
    const session = await client.createSession({
      name: 'observability-demo',
      workspace: '/tmp/observability-test',
      systemPrompt: 'You are a helpful coding assistant.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);

    console.log('  Generating responses to create metrics data...');
    await client.generate(sessionId, [
      { role: 'ROLE_USER', content: 'Write a hello world in Python' },
    ]);
    await client.generate(sessionId, [
      { role: 'ROLE_USER', content: 'Now write it in Rust' },
    ]);
    console.log('✓ Activity generated');
    console.log();

    // Tool metrics
    console.log('2. Getting tool metrics (all tools)...');
    const metrics = await client.getToolMetrics(sessionId);
    console.log('✓ Tool metrics:');
    console.log(`  Total calls: ${metrics.totalCalls}`);
    console.log(`  Total duration: ${metrics.totalDurationMs}ms`);
    console.log();

    if (metrics.tools?.length) {
      console.log('  Per-tool breakdown:');
      for (const tool of metrics.tools) {
        const successRate =
          tool.callCount > 0
            ? `${((tool.successCount / tool.callCount) * 100).toFixed(0)}%`
            : 'N/A';
        console.log(`  - ${tool.toolName}:`);
        console.log(`      Calls: ${tool.callCount} (success: ${successRate})`);
        console.log(
          `      Duration: avg=${tool.avgDurationMs}ms, ` +
            `min=${tool.minDurationMs}ms, max=${tool.maxDurationMs}ms`,
        );
      }
      console.log();
    }

    // Filter by specific tool
    console.log('3. Getting metrics for a specific tool...');
    const bashMetrics = await client.getToolMetrics(sessionId, 'bash');
    if (bashMetrics.tools?.length) {
      const tool = bashMetrics.tools[0];
      console.log(
        `✓ Bash tool: ${tool.callCount} calls, ` +
          `${tool.successCount} success, ${tool.failureCount} failures`,
      );
    } else {
      console.log('  No bash tool usage recorded');
    }
    console.log();

    // Cost summary
    console.log('4. Getting cost summary...');
    const cost = await client.getCostSummary({ sessionId });
    console.log('✓ Cost summary:');
    console.log(`  Total cost: $${cost.totalCostUsd?.toFixed(6)}`);
    console.log(`  Total tokens: ${cost.totalTokens}`);
    console.log(`    Prompt: ${cost.totalPromptTokens}`);
    console.log(`    Completion: ${cost.totalCompletionTokens}`);
    console.log(`  API calls: ${cost.callCount}`);
    console.log();

    // Per-model breakdown
    if (cost.byModel?.length) {
      console.log('  Per-model breakdown:');
      for (const model of cost.byModel) {
        console.log(`  - ${model.model}:`);
        console.log(`      Cost: $${model.costUsd?.toFixed(6)}`);
        console.log(
          `      Tokens: ${model.promptTokens} prompt + ${model.completionTokens} completion`,
        );
        console.log(`      Calls: ${model.callCount}`);
      }
      console.log();
    }

    // Per-day breakdown
    if (cost.byDay?.length) {
      console.log('  Per-day breakdown:');
      for (const day of cost.byDay) {
        console.log(`  - ${day.date}: $${day.costUsd?.toFixed(6)} (${day.callCount} calls)`);
      }
      console.log();
    }

    // Cross-session cost summary
    console.log('5. Getting cross-session cost summary...');
    const totalCost = await client.getCostSummary();
    console.log(`✓ All sessions: $${totalCost.totalCostUsd?.toFixed(6)}, ${totalCost.callCount} calls`);
    console.log();

    // Filter by model
    console.log('6. Getting cost for specific model...');
    const modelCost = await client.getCostSummary({ model: 'claude-sonnet-4-20250514' });
    console.log(`✓ Claude Sonnet 4 cost: $${modelCost.totalCostUsd?.toFixed(6)}`);
    console.log();

    // Filter by date range
    console.log('7. Getting cost for date range...');
    const dateCost = await client.getCostSummary({
      startDate: '2025-01-01',
      endDate: '2025-12-31',
    });
    console.log(`✓ 2025 cost: $${dateCost.totalCostUsd?.toFixed(6)} (${dateCost.callCount} calls)`);
    console.log();

    // Clean up
    console.log('8. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Observability example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

observabilityExample();
