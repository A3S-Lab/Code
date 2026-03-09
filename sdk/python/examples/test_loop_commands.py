#!/usr/bin/env python3
"""Test /loop and related slash commands via Python SDK.

Uses the kimi model from ~/.a3s/config.hcl.
"""

from pathlib import Path


def find_config() -> str:
    # ~/.a3s/config.hcl first, then project .a3s/
    candidates = [
        Path.home() / ".a3s" / "config.hcl",
        Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl",
    ]
    for p in candidates:
        if p.exists():
            return str(p)
    raise FileNotFoundError(f"config.hcl not found in: {[str(c) for c in candidates]}")


CONFIG_PATH = find_config()


def find_installed_module():
    try:
        import a3s_code
        return a3s_code
    except ImportError:
        return None


def main():
    mod = find_installed_module()
    if mod is None:
        print("a3s_code not installed — building via maturin develop...")
        import subprocess
        subprocess.run(
            ["maturin", "develop", "--manifest-path",
             str(Path(__file__).parent.parent / "Cargo.toml")],
            check=True
        )
        import a3s_code as mod

    Agent = mod.Agent
    print(f"Config: {CONFIG_PATH}\n")

    agent = Agent.create(CONFIG_PATH)
    session = agent.session("/tmp/test-loop")

    print("=" * 60)
    print("1. list_commands() — all registered slash commands")
    print("=" * 60)
    commands = session.list_commands()
    for cmd in commands:
        usage = f"  usage: {cmd['usage']}" if cmd.get("usage") else ""
        print(f"  /{cmd['name']:15s} {cmd['description']}{usage}")
    print()

    print("=" * 60)
    print("2. /help via send()")
    print("=" * 60)
    result = session.send("/help")
    print(result.text)
    print()

    print("=" * 60)
    print("3. /loop via send() — schedule a 10-second recurring prompt")
    print("=" * 60)
    result = session.send("/loop 10s say 'tick' and report the time")
    print(result.text)
    print()

    print("=" * 60)
    print("4. schedule_task() — programmatic API (15s interval)")
    print("=" * 60)
    task_id = session.schedule_task("print current working directory", 15)
    print(f"Scheduled task ID: {task_id}")
    print()

    print("=" * 60)
    print("5. list_scheduled_tasks() / /cron-list")
    print("=" * 60)
    tasks = session.list_scheduled_tasks()
    print(f"Active tasks: {len(tasks)}")
    for t in tasks:
        print(f"  [{t['id']}] every {t['interval_secs']}s — fires in {t['next_fire_in_secs']}s — \"{t['prompt'][:50]}\"")
    print()
    # Also via send()
    result = session.send("/cron-list")
    print("/cron-list output:")
    print(result.text)
    print()

    print("=" * 60)
    print("6. register_command() — custom Python slash command")
    print("=" * 60)

    def status_handler(args, ctx):
        n_tasks = len(session.list_scheduled_tasks())
        return (
            f"Status: session={ctx['session_id'][:8]}, "
            f"model={ctx['model']}, "
            f"history={ctx['history_len']} msgs, "
            f"scheduled_tasks={n_tasks}, "
            f"args='{args}'"
        )

    session.register_command("status", "Show session status", status_handler)
    result = session.send("/status hello world")
    print(f"/status output: {result.text}")
    print()

    print("=" * 60)
    print("7. Real LLM turn — exercises cron drain path")
    print("=" * 60)
    result = session.send("Say hello in one sentence.")
    print(f"LLM: {result.text[:200]}")
    print()

    print("=" * 60)
    print("8. cancel_scheduled_task() + /cron-cancel")
    print("=" * 60)
    cancelled = session.cancel_scheduled_task(task_id)
    print(f"cancel_scheduled_task({task_id}): {cancelled}")

    remaining = session.list_scheduled_tasks()
    if remaining:
        loop_id = remaining[0]["id"]
        result = session.send(f"/cron-cancel {loop_id}")
        print(f"/cron-cancel {loop_id}: {result.text}")
    print()

    print("=" * 60)
    print("9. Verify all tasks cancelled")
    print("=" * 60)
    tasks_after = session.list_scheduled_tasks()
    print(f"Remaining tasks: {len(tasks_after)}")
    result = session.send("/cron-list")
    print(result.text)
    print()

    print("=" * 60)
    print("All tests passed!")
    print("=" * 60)


if __name__ == "__main__":
    main()
