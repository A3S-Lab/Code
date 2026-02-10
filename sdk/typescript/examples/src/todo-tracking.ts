/**
 * Todo/Task Tracking Example
 *
 * Demonstrates how to use the built-in task tracking system:
 * - Create and manage task lists
 * - Track task status (pending/in_progress/completed/cancelled)
 * - Set task priorities
 * - Agent interaction with tasks
 */

import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function runTodoTrackingExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Todo/Task Tracking Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'Project Management Session',
      workspace: '/tmp/workspace',
      systemPrompt: 'You are a project management assistant that helps track and complete tasks.'
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Example 2: Set initial task list
    console.log('2. Setting initial task list...');
    await client.setTodos(sessionId, [
      {
        id: '1',
        content: 'Implement user authentication',
        status: 'in_progress',
        priority: 'high'
      },
      {
        id: '2',
        content: 'Write unit tests for auth module',
        status: 'pending',
        priority: 'high'
      },
      {
        id: '3',
        content: 'Update API documentation',
        status: 'pending',
        priority: 'medium'
      },
      {
        id: '4',
        content: 'Refactor database queries',
        status: 'pending',
        priority: 'medium'
      },
      {
        id: '5',
        content: 'Add logging to error handlers',
        status: 'pending',
        priority: 'low'
      }
    ]);
    console.log('✓ Task list created with 5 tasks');
    console.log();

    // Example 3: Get and display tasks
    console.log('3. Getting current task list...');
    let todos = await client.getTodos(sessionId);
    console.log(`✓ Total tasks: ${todos.todos.length}`);
    console.log();

    console.log('Current tasks:');
    for (const todo of todos.todos) {
      const statusIcon = {
        'pending': '⏳',
        'in_progress': '🔄',
        'completed': '✅',
        'cancelled': '❌'
      }[todo.status] || '❓';

      const priorityColor = {
        'high': '🔴',
        'medium': '🟡',
        'low': '🟢'
      }[todo.priority] || '⚪';

      console.log(`  ${statusIcon} [${todo.id}] ${todo.content}`);
      console.log(`     Priority: ${priorityColor} ${todo.priority} | Status: ${todo.status}`);
    }
    console.log();

    // Example 4: Agent queries tasks
    console.log('4. Agent querying pending tasks...');
    const queryResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'What tasks are currently pending?'
      }]
    });
    console.log('✓ Agent response:');
    console.log(`  ${queryResponse.content.substring(0, 200)}...`);
    console.log();

    // Example 5: Update task status
    console.log('5. Marking task 1 as completed...');
    await client.setTodos(sessionId, [
      {
        id: '1',
        content: 'Implement user authentication',
        status: 'completed',  // Changed from in_progress
        priority: 'high'
      },
      {
        id: '2',
        content: 'Write unit tests for auth module',
        status: 'in_progress',  // Started working on this
        priority: 'high'
      },
      {
        id: '3',
        content: 'Update API documentation',
        status: 'pending',
        priority: 'medium'
      },
      {
        id: '4',
        content: 'Refactor database queries',
        status: 'pending',
        priority: 'medium'
      },
      {
        id: '5',
        content: 'Add logging to error handlers',
        status: 'pending',
        priority: 'low'
      }
    ]);
    console.log('✓ Task statuses updated');
    console.log();

    // Example 6: Agent updates tasks
    console.log('6. Agent updating task status...');
    const updateResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'I finished writing the unit tests. Please mark task 2 as completed.'
      }]
    });
    console.log('✓ Agent response:');
    console.log(`  ${updateResponse.content.substring(0, 200)}...`);
    console.log();

    // Get updated tasks
    todos = await client.getTodos(sessionId);
    console.log('Updated task list:');
    for (const todo of todos.todos) {
      if (todo.status === 'completed') {
        console.log(`  ✅ [${todo.id}] ${todo.content} (${todo.priority})`);
      } else if (todo.status === 'in_progress') {
        console.log(`  🔄 [${todo.id}] ${todo.content} (${todo.priority})`);
      } else {
        console.log(`  ⏳ [${todo.id}] ${todo.content} (${todo.priority})`);
      }
    }
    console.log();

    // Example 7: Add new task
    console.log('7. Adding new task...');
    const currentTodos = await client.getTodos(sessionId);
    await client.setTodos(sessionId, [
      ...currentTodos.todos,
      {
        id: '6',
        content: 'Deploy to staging environment',
        status: 'pending',
        priority: 'high'
      }
    ]);
    console.log('✓ New task added: Deploy to staging environment');
    console.log();

    // Example 8: Cancel a task
    console.log('8. Cancelling low priority task...');
    todos = await client.getTodos(sessionId);
    const updatedTodos = todos.todos.map(todo =>
      todo.id === '5'
        ? { ...todo, status: 'cancelled' }
        : todo
    );
    await client.setTodos(sessionId, updatedTodos);
    console.log('✓ Task 5 cancelled');
    console.log();

    // Example 9: Get task statistics
    console.log('9. Task statistics...');
    todos = await client.getTodos(sessionId);
    const stats = {
      total: todos.todos.length,
      completed: todos.todos.filter(t => t.status === 'completed').length,
      inProgress: todos.todos.filter(t => t.status === 'in_progress').length,
      pending: todos.todos.filter(t => t.status === 'pending').length,
      cancelled: todos.todos.filter(t => t.status === 'cancelled').length,
      high: todos.todos.filter(t => t.priority === 'high').length,
      medium: todos.todos.filter(t => t.priority === 'medium').length,
      low: todos.todos.filter(t => t.priority === 'low').length
    };

    console.log('✓ Statistics:');
    console.log(`  Total tasks: ${stats.total}`);
    console.log(`  Completed: ${stats.completed}`);
    console.log(`  In Progress: ${stats.inProgress}`);
    console.log(`  Pending: ${stats.pending}`);
    console.log(`  Cancelled: ${stats.cancelled}`);
    console.log();
    console.log('  By priority:');
    console.log(`    High: ${stats.high}`);
    console.log(`    Medium: ${stats.medium}`);
    console.log(`    Low: ${stats.low}`);
    console.log();

    // Example 10: Agent provides task summary
    console.log('10. Agent providing task summary...');
    const summaryResponse = await client.generate(sessionId, {
      messages: [{
        role: 'user',
        content: 'Give me a summary of our progress. What tasks are done and what\'s next?'
      }]
    });
    console.log('✓ Agent summary:');
    console.log(`  ${summaryResponse.content}`);
    console.log();

    // Cleanup
    console.log('11. Cleanup...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Todo/Task Tracking Example Complete');
    console.log('='.repeat(60));
    console.log();
    console.log('Use cases:');
    console.log('  ✓ Project management');
    console.log('  ✓ Sprint planning');
    console.log('  ✓ Code review checklists');
    console.log('  ✓ Debugging task lists');
    console.log('  ✓ Feature implementation tracking');

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runTodoTrackingExample().catch(console.error);
