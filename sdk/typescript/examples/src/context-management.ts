/**
 * Context Management Example
 *
 * Demonstrates how to manage session context and token usage:
 * - Monitor context usage
 * - Compact context when approaching limits
 * - Clear context for fresh starts
 * - Auto-compact configuration
 */

import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

// Helper to format token counts
function formatTokens(tokens: number): string {
  if (tokens >= 1000000) {
    return `${(tokens / 1000000).toFixed(2)}M`;
  } else if (tokens >= 1000) {
    return `${(tokens / 1000).toFixed(1)}K`;
  }
  return tokens.toString();
}

// Helper to create progress bar
function progressBar(current: number, max: number, width: number = 30): string {
  const percentage = Math.min(current / max, 1);
  const filled = Math.round(percentage * width);
  const empty = width - filled;
  const bar = '█'.repeat(filled) + '░'.repeat(empty);
  return `[${bar}] ${(percentage * 100).toFixed(1)}%`;
}

async function runContextManagementExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Context Management Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Example 1: Create session with auto-compact enabled
    console.log('1. Creating session with auto-compact...');
    const session = await client.createSession({
      name: 'Context Management Demo',
      workspace: '/tmp/workspace',
      maxContextLength: 200000,  // 200K token limit
      autoCompact: true,  // Enable automatic compaction
      systemPrompt: 'You are a helpful assistant. Keep track of our conversation context.'
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log('  Max context: 200K tokens');
    console.log('  Auto-compact: enabled');
    console.log();

    // Example 2: Check initial context usage
    console.log('2. Checking initial context usage...');
    let usage = await client.getContextUsage(sessionId);
    console.log('✓ Initial context usage:');
    console.log(`  Total tokens: ${formatTokens(usage.totalTokens)}`);
    console.log(`  Prompt tokens: ${formatTokens(usage.promptTokens)}`);
    console.log(`  Completion tokens: ${formatTokens(usage.completionTokens)}`);
    console.log(`  Message count: ${usage.messageCount}`);
    console.log(`  Usage: ${progressBar(usage.totalTokens, 200000)}`);
    console.log();

    // Example 3: Generate some conversation to build up context
    console.log('3. Building up conversation context...');
    const conversations = [
      'Tell me about the history of programming languages.',
      'What are the key differences between functional and object-oriented programming?',
      'Explain the concept of design patterns in software engineering.',
      'What are microservices and how do they differ from monolithic architectures?',
      'Describe the principles of clean code and why they matter.'
    ];

    for (let i = 0; i < conversations.length; i++) {
      console.log(`  [${i + 1}/${conversations.length}] Sending message...`);
      await client.generate(sessionId, {
        messages: [{
          role: 'user',
          content: conversations[i]
        }]
      });

      // Check usage after each message
      usage = await client.getContextUsage(sessionId);
      console.log(`      Tokens: ${formatTokens(usage.totalTokens)} ${progressBar(usage.totalTokens, 200000, 20)}`);
    }
    console.log();

    // Example 4: Monitor context usage
    console.log('4. Current context status...');
    usage = await client.getContextUsage(sessionId);
    const usagePercent = (usage.totalTokens / 200000) * 100;

    console.log('✓ Context usage:');
    console.log(`  Total tokens: ${formatTokens(usage.totalTokens)} / 200K`);
    console.log(`  Messages: ${usage.messageCount}`);
    console.log(`  Usage: ${progressBar(usage.totalTokens, 200000)}`);
    console.log();

    if (usagePercent > 75) {
      console.log('⚠️  Context is getting full (>75%)');
    } else if (usagePercent > 50) {
      console.log('ℹ️  Context is moderately used (>50%)');
    } else {
      console.log('✓ Context has plenty of room (<50%)');
    }
    console.log();

    // Example 5: Manual context compaction
    console.log('5. Manually compacting context...');
    console.log('  Before compaction:');
    console.log(`    Tokens: ${formatTokens(usage.totalTokens)}`);
    console.log(`    Messages: ${usage.messageCount}`);

    const compactResult = await client.compactContext(sessionId);
    console.log('  After compaction:');
    console.log(`    Tokens: ${formatTokens(compactResult.after.totalTokens)}`);
    console.log(`    Messages: ${compactResult.after.messageCount}`);
    console.log(`    Saved: ${formatTokens(compactResult.before.totalTokens - compactResult.after.totalTokens)} tokens`);
    console.log();

    // Example 6: Continue conversation after compaction
    console.log('6. Continuing conversation after compaction...');
    const response = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Can you summarize what we discussed earlier?'
      }]
    });
    console.log('✓ Agent can still recall context:');
    console.log(`  ${response.content.substring(0, 200)}...`);
    console.log();

    // Example 7: Create session without auto-compact for comparison
    console.log('7. Creating session without auto-compact...');
    const manualSession = await client.createSession({
      name: 'Manual Context Management',
      workspace: '/tmp/workspace',
      maxContextLength: 200000,
      autoCompact: false,  // Disable auto-compact
      systemPrompt: 'You are a helpful assistant.'
    });
    console.log(`✓ Session created: ${manualSession.sessionId}`);
    console.log('  Auto-compact: disabled');
    console.log('  Note: You must manually compact or clear context');
    console.log();

    // Example 8: Clear context completely
    console.log('8. Clearing context completely...');
    console.log('  Before clear:');
    usage = await client.getContextUsage(sessionId);
    console.log(`    Tokens: ${formatTokens(usage.totalTokens)}`);
    console.log(`    Messages: ${usage.messageCount}`);

    await client.clearContext(sessionId);

    console.log('  After clear:');
    usage = await client.getContextUsage(sessionId);
    console.log(`    Tokens: ${formatTokens(usage.totalTokens)}`);
    console.log(`    Messages: ${usage.messageCount}`);
    console.log('  Note: System prompt is preserved');
    console.log();

    // Example 9: Context monitoring loop (demonstration)
    console.log('9. Context monitoring demonstration...');
    console.log('  Setting up monitoring (checking every 2 seconds)...');

    // Simulate monitoring for a few iterations
    for (let i = 0; i < 3; i++) {
      await new Promise(resolve => setTimeout(resolve, 2000));

      usage = await client.getContextUsage(sessionId);
      const percent = (usage.totalTokens / 200000) * 100;

      console.log(`  [Monitor] Tokens: ${formatTokens(usage.totalTokens)} (${percent.toFixed(1)}%)`);

      // Auto-compact if over 90%
      if (percent > 90) {
        console.log('  [Monitor] ⚠️ Context over 90%, auto-compacting...');
        await client.compactContext(sessionId);
      }
    }
    console.log();

    // Cleanup
    console.log('10. Cleanup...');
    await client.destroySession(sessionId);
    await client.destroySession(manualSession.sessionId);
    console.log('✓ Sessions destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Context Management Example Complete');
    console.log('='.repeat(60));
    console.log();
    console.log('Best practices:');
    console.log('  ✓ Enable auto-compact for long-running sessions');
    console.log('  ✓ Monitor context usage regularly');
    console.log('  ✓ Compact at 75-80% to avoid hitting limits');
    console.log('  ✓ Clear context for completely fresh starts');
    console.log('  ✓ Use appropriate max_context_length for your model');

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runContextManagementExample().catch(console.error);
