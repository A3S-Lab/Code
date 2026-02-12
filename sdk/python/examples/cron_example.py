"""
Cron Scheduling Example

Demonstrates how to manage scheduled tasks via the cron system:
- Creating cron jobs with standard cron syntax
- Parsing natural language schedules
- Job lifecycle (pause, resume, update, delete)
- Manual job execution
- Viewing execution history
"""

import asyncio
from a3s_code import A3sClient


async def cron_example():
    print("=" * 60)
    print("Cron Scheduling Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # =====================================================================
        # Parse Natural Language Schedule
        # =====================================================================
        print("1. Parsing natural language schedules...")
        schedules = [
            "every 5 minutes",
            "every day at 2am",
            "every Monday at 9:30",
        ]
        for text in schedules:
            result = await client.parse_cron_schedule(text)
            cron_expr = result.get("expression", "N/A")
            description = result.get("description", "")
            print(f"  '{text}' → {cron_expr}  ({description})")
        print()

        # =====================================================================
        # Create Cron Jobs
        # =====================================================================
        print("2. Creating cron jobs...")

        # Job 1: Run every 5 minutes
        job1 = await client.create_cron_job(
            name="health-check",
            schedule="*/5 * * * *",
            command="curl -s http://localhost:8080/health",
            timeout_ms=10000,
        )
        job1_id = job1.get("id", "")
        print(f"✓ Created '{job1.get('name')}': {job1.get('schedule')} (id: {job1_id})")

        # Job 2: Daily backup at 2am
        job2 = await client.create_cron_job(
            name="daily-backup",
            schedule="0 2 * * *",
            command="tar -czf /tmp/backup-$(date +%Y%m%d).tar.gz /data",
            timeout_ms=300000,
        )
        job2_id = job2.get("id", "")
        print(f"✓ Created '{job2.get('name')}': {job2.get('schedule')} (id: {job2_id})")

        # Job 3: Weekly report
        job3 = await client.create_cron_job(
            name="weekly-report",
            schedule="0 9 * * 1",
            command="python /scripts/generate_report.py",
        )
        job3_id = job3.get("id", "")
        print(f"✓ Created '{job3.get('name')}': {job3.get('schedule')} (id: {job3_id})")
        print()

        # =====================================================================
        # List All Jobs
        # =====================================================================
        print("3. Listing all cron jobs...")
        jobs = await client.list_cron_jobs()
        print(f"✓ Found {len(jobs)} jobs:")
        for job in jobs:
            print(f"  - {job.name} [{job.status.name}]: {job.schedule}")
            print(f"    Command: {job.command[:60]}...")
            print(f"    Runs: {job.run_count} success, {job.fail_count} failed")
        print()

        # =====================================================================
        # Get Job Details
        # =====================================================================
        print("4. Getting job details...")

        # By ID
        job_detail = await client.get_cron_job(id=job1_id)
        if job_detail:
            print(f"✓ Job by ID: {job_detail.name}")
            print(f"  Schedule: {job_detail.schedule}")
            print(f"  Timeout: {job_detail.timeout_ms}ms")
            if job_detail.next_run:
                print(f"  Next run: {job_detail.next_run}")

        # By name
        job_by_name = await client.get_cron_job(name="daily-backup")
        if job_by_name:
            print(f"✓ Job by name: {job_by_name.name} (id: {job_by_name.id})")
        print()

        # =====================================================================
        # Update a Job
        # =====================================================================
        print("5. Updating job schedule...")
        updated = await client.update_cron_job(
            id=job1_id,
            schedule="*/10 * * * *",  # Change to every 10 minutes
            timeout_ms=15000,
        )
        print(f"✓ Updated '{updated.get('name')}': schedule → {updated.get('schedule')}")
        print()

        # =====================================================================
        # Pause and Resume
        # =====================================================================
        print("6. Pausing job...")
        pause_result = await client.pause_cron_job(job2_id)
        print(f"✓ Paused: {pause_result.get('name')} → status: {pause_result.get('status')}")

        print("   Resuming job...")
        resume_result = await client.resume_cron_job(job2_id)
        print(f"✓ Resumed: {resume_result.get('name')} → status: {resume_result.get('status')}")
        print()

        # =====================================================================
        # Manual Execution
        # =====================================================================
        print("7. Running job manually...")
        run_result = await client.run_cron_job(job1_id)
        print(f"✓ Execution result:")
        print(f"  Status: {run_result.get('status')}")
        print(f"  Exit code: {run_result.get('exit_code')}")
        stdout = run_result.get("stdout", "")
        if stdout:
            print(f"  Output: {stdout[:200]}")
        stderr = run_result.get("stderr", "")
        if stderr:
            print(f"  Stderr: {stderr[:200]}")
        print()

        # =====================================================================
        # Execution History
        # =====================================================================
        print("8. Getting execution history...")
        history = await client.get_cron_history(job1_id, limit=5)
        print(f"✓ Found {len(history)} executions:")
        for exec_record in history:
            duration = f"{exec_record.duration_ms}ms" if exec_record.duration_ms else "N/A"
            print(f"  - [{exec_record.status.name}] duration={duration}, exit={exec_record.exit_code}")
            if exec_record.stdout:
                print(f"    stdout: {exec_record.stdout[:80]}")
        print()

        # =====================================================================
        # Delete a Job
        # =====================================================================
        print("9. Deleting jobs...")
        for jid in [job1_id, job2_id, job3_id]:
            result = await client.delete_cron_job(jid)
            print(f"✓ Deleted: {result.get('name', jid)}")

        # Verify deletion
        remaining = await client.list_cron_jobs()
        print(f"  Remaining jobs: {len(remaining)}")
        print()

        print("=" * 60)
        print("Cron example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(cron_example())
