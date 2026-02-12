/**
 * Cron Scheduling Example
 *
 * Demonstrates how to manage scheduled tasks via the cron system:
 * - Creating cron jobs with standard cron syntax
 * - Parsing natural language schedules
 * - Job lifecycle (pause, resume, update, delete)
 * - Manual job execution
 * - Viewing execution history
 */

import { A3sClient } from '@a3s-lab/code';

async function cronExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Cron Scheduling Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Parse natural language schedules
    console.log('1. Parsing natural language schedules...');
    const schedules = ['every 5 minutes', 'every day at 2am', 'every Monday at 9:30'];
    for (const text of schedules) {
      const result = await client.parseCronSchedule(text);
      console.log(`  '${text}' → ${result.cronExpression}  (${result.description})`);
    }
    console.log();

    // Create cron jobs
    console.log('2. Creating cron jobs...');

    const job1 = await client.createCronJob(
      'health-check',
      '*/5 * * * *',
      'curl -s http://localhost:8080/health',
      10000,
    );
    const job1Id = job1.id || '';
    console.log(`✓ Created '${job1.name}': ${job1.schedule} (id: ${job1Id})`);

    const job2 = await client.createCronJob(
      'daily-backup',
      '0 2 * * *',
      'tar -czf /tmp/backup-$(date +%Y%m%d).tar.gz /data',
      300000,
    );
    const job2Id = job2.id || '';
    console.log(`✓ Created '${job2.name}': ${job2.schedule} (id: ${job2Id})`);

    const job3 = await client.createCronJob(
      'weekly-report',
      '0 9 * * 1',
      'python /scripts/generate_report.py',
    );
    const job3Id = job3.id || '';
    console.log(`✓ Created '${job3.name}': ${job3.schedule} (id: ${job3Id})`);
    console.log();

    // List all jobs
    console.log('3. Listing all cron jobs...');
    const jobs = await client.listCronJobs();
    console.log(`✓ Found ${jobs.jobs?.length || 0} jobs:`);
    for (const job of jobs.jobs || []) {
      console.log(`  - ${job.name} [${job.status}]: ${job.schedule}`);
      console.log(`    Command: ${job.command.substring(0, 60)}...`);
    }
    console.log();

    // Get job details
    console.log('4. Getting job details...');
    const jobDetail = await client.getCronJob(job1Id);
    if (jobDetail.job) {
      console.log(`✓ Job by ID: ${jobDetail.job.name}`);
      console.log(`  Schedule: ${jobDetail.job.schedule}`);
      console.log(`  Timeout: ${jobDetail.job.timeoutMs}ms`);
    }

    const jobByName = await client.getCronJob(undefined, 'daily-backup');
    if (jobByName.job) {
      console.log(`✓ Job by name: ${jobByName.job.name} (id: ${jobByName.job.id})`);
    }
    console.log();

    // Update a job
    console.log('5. Updating job schedule...');
    const updated = await client.updateCronJob(job1Id, '*/10 * * * *', undefined, 15000);
    console.log(`✓ Updated: schedule → ${updated.schedule}`);
    console.log();

    // Pause and resume
    console.log('6. Pausing job...');
    const pauseResult = await client.pauseCronJob(job2Id);
    console.log(`✓ Paused: ${pauseResult.name} → status: ${pauseResult.status}`);

    console.log('   Resuming job...');
    const resumeResult = await client.resumeCronJob(job2Id);
    console.log(`✓ Resumed: ${resumeResult.name} → status: ${resumeResult.status}`);
    console.log();

    // Manual execution
    console.log('7. Running job manually...');
    const runResult = await client.runCronJob(job1Id);
    console.log('✓ Execution result:');
    console.log(`  Status: ${runResult.status}`);
    console.log(`  Exit code: ${runResult.exitCode}`);
    if (runResult.stdout) {
      console.log(`  Output: ${runResult.stdout.substring(0, 200)}`);
    }
    console.log();

    // Execution history
    console.log('8. Getting execution history...');
    const history = await client.getCronHistory(job1Id, 5);
    console.log(`✓ Found ${history.executions?.length || 0} executions:`);
    for (const exec of history.executions || []) {
      const duration = exec.durationMs ? `${exec.durationMs}ms` : 'N/A';
      console.log(`  - [${exec.status}] duration=${duration}, exit=${exec.exitCode}`);
    }
    console.log();

    // Delete jobs
    console.log('9. Deleting jobs...');
    for (const jid of [job1Id, job2Id, job3Id]) {
      const result = await client.deleteCronJob(jid);
      console.log(`✓ Deleted: ${result.name || jid}`);
    }

    const remaining = await client.listCronJobs();
    console.log(`  Remaining jobs: ${remaining.jobs?.length || 0}`);
    console.log();

    console.log('='.repeat(60));
    console.log('Cron example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

cronExample();
