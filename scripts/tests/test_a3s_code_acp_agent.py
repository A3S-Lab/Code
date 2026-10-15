"""The Harbor ACP adapter installs the local build and scores finished attempts.

Run with Harbor's interpreter, e.g.
`~/.local/share/uv/tools/harbor/bin/python -m unittest scripts/tests/test_a3s_code_acp_agent.py`.
"""

import asyncio
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/terminal_bench"))

try:
    from harbor.agents.installed.acp import AcpAgent

    import a3s_code_acp_agent as adapter
except ImportError as error:  # pragma: no cover - needs Harbor installed
    raise unittest.SkipTest(f"Harbor is not importable: {error}")


class FakeEnvironment:
    def __init__(self, report=None, failing=None):
        self.report = report
        self.failing = failing
        self.commands = []
        self.uploads = []

    async def exec(self, command, **_kwargs):
        self.commands.append(command)
        if self.failing and self.failing in command:
            return SimpleNamespace(return_code=1, stdout="", stderr="boom")
        if command.startswith("cat ") and self.report is not None:
            return SimpleNamespace(return_code=0, stdout=self.report, stderr="")
        if command.startswith("cat "):
            return SimpleNamespace(return_code=1, stdout="", stderr="missing")
        return SimpleNamespace(return_code=0, stdout="", stderr="")

    async def upload_file(self, source, target):
        self.uploads.append((Path(source), target))

    async def upload_dir(self, source, target):
        self.uploads.append((Path(source), target))


class AcpAdapterTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binary = self.root / "a3s-code-acp"
        self.binary.write_text("binary")
        self.config = self.root / "config.acl"
        self.config.write_text("acl")

    def agent(self, **extra_env):
        env = {
            adapter.A3SCodeAcpAgent.BINARY_ENV: str(self.binary),
            adapter.A3SCodeAcpAgent.CONFIG_ENV: str(self.config),
            **extra_env,
        }
        return adapter.A3SCodeAcpAgent(logs_dir=self.root / "logs", extra_env=env)

    def launcher(self, agent):
        kind, target = agent._select_distribution("linux-x86_64")
        return agent._build_launcher_script(kind, target)

    def test_launcher_runs_unattended_with_budget_and_no_credentials(self):
        with mock.patch.dict(os.environ, {"DEEPSEEK_API_KEY": "secret-key"}):
            agent = self.agent(A3S_CODE_TERMINAL_BENCH_MAX_EXECUTION_MS="1200000")
        script = self.launcher(agent)
        self.assertIn("/opt/harbor-acp-agent/dist/a3s-code-acp --unattended", script)
        self.assertIn(f"--config {adapter.REMOTE_CONFIG}", script)
        self.assertIn(f"--result-file {adapter.RESULT_PATH}", script)
        self.assertIn("--max-execution-ms 1200000", script)
        self.assertIn("export A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX=1", script)
        self.assertNotIn("secret-key", script)
        self.assertEqual(agent.extra_env["DEEPSEEK_API_KEY"], "secret-key")

    def test_reasoning_effort_and_stream_idle_reach_the_agent(self):
        with mock.patch.dict(os.environ, {adapter.A3SCodeAcpAgent.STREAM_IDLE_ENV: "120000"}):
            agent = self.agent(A3S_CODE_ACP_REASONING_EFFORT="high")
        self.assertIn("--reasoning-effort high", self.launcher(agent))
        self.assertEqual(agent.extra_env[adapter.A3SCodeAcpAgent.STREAM_IDLE_ENV], "120000")
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop(adapter.A3SCodeAcpAgent.REASONING_EFFORT_ENV, None)
            self.assertNotIn("--reasoning-effort", self.launcher(self.agent()))

    def test_trial_arms_reach_the_agent_only_when_set(self):
        agent = self.agent(
            A3S_CODE_ACP_ENGINEERING_POLICY="engineering-v1",
            A3S_CODE_ACP_GOAL_CONTINUATION="1",
        )
        script = self.launcher(agent)
        self.assertIn("--engineering-policy engineering-v1", script)
        self.assertIn("--goal-continuation", script)
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop(adapter.A3SCodeAcpAgent.ENGINEERING_POLICY_ENV, None)
            os.environ.pop(adapter.A3SCodeAcpAgent.GOAL_CONTINUATION_ENV, None)
            script = self.launcher(self.agent())
        self.assertNotIn("--engineering-policy", script)
        self.assertNotIn("--goal-continuation", script)

    def test_launcher_omits_the_budget_flag_when_unset(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop(adapter.A3SCodeAcpAgent.EXECUTION_BUDGET_ENV, None)
            script = self.launcher(self.agent())
        self.assertNotIn("--max-execution-ms", script)

    def test_install_uploads_the_local_build_instead_of_fetching(self):
        agent = self.agent()
        environment = FakeEnvironment()
        _kind, target = agent._select_distribution("linux-x86_64")
        asyncio.run(agent._install_binary_target(environment, target))
        self.assertEqual(
            environment.uploads,
            [
                (self.binary, "/opt/harbor-acp-agent/dist/a3s-code-acp"),
                (self.config, adapter.REMOTE_CONFIG),
            ],
        )
        self.assertFalse(any("curl" in command for command in environment.commands))
        self.assertTrue(any("chmod 644 /run/a3s/config.acl" in c for c in environment.commands))

    def install(self, environment, **extra_env):
        agent = self.agent(**extra_env)
        _kind, target = agent._select_distribution("linux-x86_64")
        asyncio.run(agent._install_binary_target(environment, target))

    def test_install_sets_up_language_servers_unless_opted_out(self):
        environment = FakeEnvironment()
        self.install(environment)
        self.assertTrue(any("clangd" in command for command in environment.commands))

        environment = FakeEnvironment()
        self.install(environment, A3S_CODE_ACP_LANGUAGE_SERVERS="0")
        self.assertFalse(any("clangd" in command for command in environment.commands))

    def test_a_failed_language_server_setup_never_fails_the_install(self):
        environment = FakeEnvironment(failing="basedpyright")
        self.install(environment)
        self.assertTrue(any("basedpyright" in command for command in environment.commands))

    def test_language_server_setup_is_valid_posix_shell(self):
        import subprocess

        script = adapter.language_server_setup(adapter.DEFAULT_LANGUAGE_SERVER_BUDGET_SEC)
        subprocess.run(["sh", "-n", "-c", script], check=True)
        self.assertIn("--download-only clangd", script)

    def test_setup_fits_inside_harbors_agent_setup_deadline(self):
        timeouts = []
        agent = self.agent()

        async def record(_environment, command, **kwargs):
            if "language server" in command:
                timeouts.append(kwargs.get("timeout_sec"))

        with mock.patch.object(agent, "exec_as_root", record):
            asyncio.run(agent._install_language_servers(FakeEnvironment()))
        self.assertEqual(timeouts, [adapter.DEFAULT_LANGUAGE_SERVER_BUDGET_SEC + 60])
        self.assertLess(timeouts[0], 360)

    def test_a_prepared_bundle_replaces_the_network_install(self):
        bundle = self.root / "lsp.tar.gz"
        bundle.write_bytes(b"bundle")
        environment = FakeEnvironment()
        self.install(environment, A3S_CODE_LSP_BUNDLE=str(bundle))
        self.assertIn((bundle, "/tmp/a3s-lsp.tar.gz"), environment.uploads)
        unpack = [c for c in environment.commands if "tar -xzf /tmp/a3s-lsp.tar.gz" in c]
        self.assertEqual(len(unpack), 1)
        self.assertIn('[ -e "/usr/local/bin/${f##*/}" ]', unpack[0])
        for network in ("npm install", "apt-get", "pip install"):
            self.assertFalse(any(network in c for c in environment.commands), network)

    def test_a_configured_but_missing_bundle_is_refused(self):
        with self.assertRaises(FileNotFoundError):
            self.install(FakeEnvironment(), A3S_CODE_LSP_BUNDLE=str(self.root / "nope"))

    def test_a_runner_bundle_lands_before_harbors_install_and_gates_it(self):
        import subprocess

        bundle = self.root / "runner.tar.gz"
        bundle.write_bytes(b"bundle")
        agent = self.agent(A3S_CODE_RUNNER_BUNDLE=str(bundle))
        environment = FakeEnvironment()
        order = []

        async def harbor_install(_self, env):
            order.append(list(env.commands))

        with mock.patch.object(AcpAgent, "install", harbor_install):
            asyncio.run(agent.install(environment))
        self.assertIn((bundle, "/tmp/a3s-acp-runner.tar.gz"), environment.uploads)
        self.assertTrue(any("tar -xzf /tmp/a3s-acp-runner.tar.gz -C /opt/harbor-acp-venv" in c for c in order[0]))

        gated = agent._build_dependencies_command("binary")
        subprocess.run(["sh", "-n", "-c", gated], check=True)
        self.assertTrue(gated.startswith('if /opt/harbor-acp-venv/bin/python -c "import acp"'))
        self.assertIn("apt-get update", gated)
        plain = self.agent()._build_dependencies_command("binary")
        self.assertNotIn("import acp\" >/dev/null 2>&1; then exit 0", plain)

    def test_skills_are_shipped_only_when_configured(self):
        skills = self.root / "skills"
        skills.mkdir()
        environment = FakeEnvironment()
        self.install(environment, A3S_CODE_SKILLS_DIR=str(skills))
        self.assertIn((skills, adapter.REMOTE_SKILLS), environment.uploads)

        environment = FakeEnvironment()
        self.install(environment)
        self.assertFalse(any(t == adapter.REMOTE_SKILLS for _s, t in environment.uploads))

    def test_install_refuses_a_missing_binary(self):
        self.binary.unlink()
        agent = self.agent()
        _kind, target = agent._select_distribution("linux-x86_64")
        with self.assertRaises(FileNotFoundError):
            asyncio.run(agent._install_binary_target(FakeEnvironment(), target))

    def run_agent(self, report, runner_error=None):
        agent = self.agent()
        environment = FakeEnvironment(None if report is None else json.dumps(report))

        async def runner(*_args, **_kwargs):
            if runner_error is not None:
                raise runner_error

        context = SimpleNamespace(metadata=None)
        with mock.patch.object(AcpAgent, "run", runner):
            asyncio.run(agent.run("task", environment, context))
        return agent

    def test_a_tool_using_failure_still_reaches_the_verifier(self):
        agent = self.run_agent(
            {"outcome": "failed", "reason": "agent_error", "toolCalls": 3},
            runner_error=RuntimeError("runner exit 1"),
        )
        context = SimpleNamespace(metadata={"acp": {}})
        with mock.patch.object(AcpAgent, "populate_context_post_run"):
            agent.populate_context_post_run(context)
        self.assertEqual(context.metadata["terminal_reason"], "agent_error")
        self.assertEqual(context.metadata["tool_calls"], 3)
        self.assertEqual(context.metadata["acp"], {})

    def test_unscorable_attempts_raise(self):
        for report in (
            None,
            {"outcome": "failed", "reason": "agent_error", "toolCalls": 0},
            {"outcome": "failed", "reason": "provider_rejected", "toolCalls": 4},
        ):
            with self.subTest(report=report), self.assertRaises(RuntimeError):
                self.run_agent(report)

    def test_success_never_raises(self):
        self.run_agent({"outcome": "succeeded", "reason": "agent_end", "toolCalls": 0})


if __name__ == "__main__":
    unittest.main()
