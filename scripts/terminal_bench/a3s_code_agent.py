"""Harbor adapter for the native headless A3S Code session.

This is an evaluation adapter, not a second agent runtime. Harbor owns task
containers and verification; A3S Code owns the model loop and native tools.
"""

from __future__ import annotations

import json
import shlex
import tempfile
from pathlib import Path

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class A3SCodeAgent(BaseAgent):
    """Run the A3S Code core session against a Harbor task workspace."""

    BINARY_ENV = "A3S_CODE_TERMINAL_BENCH_BINARY"
    CONFIG_ENV = "A3S_CODE_CONFIG"
    CODEX_AUTH_ENV = "A3S_CODEX_AUTH_FILE"
    CODEX_MODEL_ENV = "A3S_CODEX_MODEL"
    CODEX_REASONING_ENV = "A3S_CODEX_REASONING_EFFORT"
    EXECUTION_BUDGET_ENV = "A3S_CODE_TERMINAL_BENCH_MAX_EXECUTION_MS"
    RESULT_PATH = "/logs/agent/a3s-code.result.json"

    @staticmethod
    def name() -> str:
        return "a3s-code"

    def version(self) -> str:
        return "8.2.2-terminal-bench"

    async def setup(self, environment: BaseEnvironment) -> None:
        binary = self._get_env(self.BINARY_ENV)
        config = self._get_env(self.CONFIG_ENV)
        codex_auth = self._get_env(self.CODEX_AUTH_ENV)
        if not binary:
            raise RuntimeError(f"{self.BINARY_ENV} must point to the built runner")
        if not config:
            raise RuntimeError(f"{self.CONFIG_ENV} must point to an ACL file")
        binary_path = Path(binary).expanduser()
        config_path = Path(config).expanduser()
        if not binary_path.is_file():
            raise FileNotFoundError(binary_path)
        if not config_path.is_file():
            raise FileNotFoundError(config_path)
        if codex_auth and not Path(codex_auth).expanduser().is_file():
            raise FileNotFoundError(Path(codex_auth).expanduser())

        await environment.exec("mkdir -p /run/a3s", user="root")
        await environment.upload_file(binary_path, "/run/a3s/terminal_bench_runner")
        await environment.upload_file(config_path, "/run/a3s/config.acl")
        if codex_auth:
            await environment.upload_file(
                Path(codex_auth).expanduser(), "/run/a3s/codex-auth.json"
            )
        await environment.exec(
            "chmod 755 /run/a3s/terminal_bench_runner && chmod 600 /run/a3s/config.acl "
            "&& if [ -f /run/a3s/codex-auth.json ]; then chmod 600 /run/a3s/codex-auth.json; fi",
            user="root",
        )

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        workdir_result = await environment.exec("pwd")
        workdir = (workdir_result.stdout or "/").strip() or "/"
        with tempfile.TemporaryDirectory(prefix="a3s-terminal-bench-") as temp_dir:
            prompt_path = Path(temp_dir) / "instruction.md"
            prompt_path.write_text(instruction, encoding="utf-8")
            await environment.upload_file(prompt_path, "/run/a3s/instruction.md")

        command = (
            "/run/a3s/terminal_bench_runner "
            "--config /run/a3s/config.acl "
            f"--workspace {shlex.quote(workdir)} "
            "--prompt-file /run/a3s/instruction.md "
            f"--result-file {shlex.quote(self.RESULT_PATH)}"
        )
        execution_budget = self._get_env(self.EXECUTION_BUDGET_ENV)
        if execution_budget:
            command += f" --max-execution-time-ms {shlex.quote(execution_budget)}"
        if self._get_env(self.CODEX_AUTH_ENV):
            model = self._get_env(self.CODEX_MODEL_ENV) or "gpt-6-astra"
            reasoning = self._get_env(self.CODEX_REASONING_ENV) or "max"
            command += (
                " --codex-auth /run/a3s/codex-auth.json"
                f" --codex-model {shlex.quote(model)}"
                f" --codex-reasoning-effort {shlex.quote(reasoning)}"
            )
        command += (
            " "
            f"> {shlex.quote(str(self.environment_logs_dir / 'a3s-code.stdout.txt'))} "
            f"2> {shlex.quote(str(self.environment_logs_dir / 'a3s-code.stderr.txt'))}"
        )
        result = await environment.exec(
            command,
            cwd=workdir,
            env={
                "A3S_CODE_TRAJECTORY_PATH": str(
                    self.environment_logs_dir / "a3s-code.trajectory.jsonl"
                ),
            },
        )
        report = None
        report_read_error = None
        report_result = await environment.exec(
            f"cat {shlex.quote(self.RESULT_PATH)}", cwd=workdir
        )
        if report_result.return_code == 0 and report_result.stdout:
            try:
                report = json.loads(report_result.stdout)
            except json.JSONDecodeError as error:
                report_read_error = f"invalid terminal result JSON: {error}"
        else:
            report_read_error = "terminal result file was not readable"
        context.metadata = {
            "runner": self.name(),
            "runner_version": self.version(),
            "exit_code": result.return_code,
            "result_file": self.RESULT_PATH,
        }
        if report is not None:
            context.metadata.update(
                {
                    "terminal_outcome": report.get("outcome"),
                    "terminal_reason": report.get("reason"),
                    "terminal_event": report.get("terminalEvent"),
                    "duration_ms": report.get("durationMs"),
                    "turns": report.get("turns"),
                    "tool_calls": report.get("toolCalls"),
                    "error_count": report.get("errorCount"),
                    "provider": report.get("provider"),
                    "provider_status": report.get("providerStatus"),
                }
            )
        elif report_read_error is not None:
            context.metadata["result_read_error"] = report_read_error
        if result.return_code != 0:
            reason = (report or {}).get("reason", "unknown")
            raise RuntimeError(
                "A3S Code runner failed with exit code "
                f"{result.return_code} ({reason}); inspect a3s-code.stderr.txt "
                "and a3s-code.result.json"
            )
