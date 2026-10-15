"""Harbor adapter for `a3s-code-acp --unattended`.

Harbor's stock ACP agent drives the session; this subclass only installs the
locally built binary and ACL instead of fetching a release archive, and keeps
the native adapter's rule for when a finished attempt still reaches the
verifier.
"""

from __future__ import annotations

import json
import os
import shlex
from pathlib import Path
from typing import Any

from harbor.agents.installed.acp import AcpAgent, AcpBinaryTarget
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from a3s_code_agent import _forwarded_provider_env

REMOTE_CONFIG = "/run/a3s/config.acl"
# List this path in the evaluation ACL's `skill_dirs` to load shipped skills.
REMOTE_SKILLS = "/run/a3s/skills"
RESULT_PATH = "/logs/agent/a3s-code.result.json"
COMMAND = "a3s-code-acp"
LSP_PREFIX = "/opt/a3s-lsp"

# Harbor's whole agent setup shares one deadline (360 s by default). The
# network install below is the fallback when no prepared bundle is configured.
DEFAULT_LANGUAGE_SERVER_BUDGET_SEC = 150


def language_server_setup(budget_sec: int) -> str:
    """Best-effort install under one deadline. Only network and private-prefix
    steps are interruptible; apt installs from its finished download, so a cut
    never leaves dpkg half-configured for the agent. A skipped step leaves
    Code Intelligence reporting that language as unavailable."""
    return f"""
set +e
export DEBIAN_FRONTEND=noninteractive
have() {{ command -v "$1" >/dev/null 2>&1; }}
have timeout || {{ echo "language servers skipped: no timeout(1)"; exit 0; }}
deadline=$(( $(date +%s) + {budget_sec} ))
bounded() {{
  left=$(( deadline - $(date +%s) ))
  [ "$left" -gt 5 ] || return 124
  timeout "$left" "$@"
}}
mkdir -p {LSP_PREFIX}
if have npm; then
  bounded npm install --silent --no-fund --no-audit --prefix {LSP_PREFIX}/npm \\
    basedpyright typescript-language-server typescript \\
    && ln -sf {LSP_PREFIX}/npm/node_modules/.bin/basedpyright-langserver \\
      {LSP_PREFIX}/npm/node_modules/.bin/typescript-language-server /usr/local/bin/
fi
if ! have basedpyright-langserver && ! have pyright-langserver && have python3; then
  bounded python3 -m venv {LSP_PREFIX}/venv \\
    && bounded {LSP_PREFIX}/venv/bin/pip install -q basedpyright \\
    && ln -sf {LSP_PREFIX}/venv/bin/basedpyright-langserver /usr/local/bin/
fi
if ! have clangd; then
  if have apt-get; then
    bounded apt-get update -qq \\
      && bounded apt-get install -y -qq --no-install-recommends --download-only clangd \\
      && apt-get install -y -qq --no-install-recommends clangd
  elif have apk; then
    bounded apk add --no-cache clang-extra-tools
  fi
fi
for server in basedpyright-langserver pyright-langserver typescript-language-server \\
  clangd rust-analyzer; do
  have "$server" && echo "language server ready: $server"
done
exit 0
"""


def registry_entry(platform: str) -> dict[str, Any]:
    """Registry record for the uploaded binary. The archive is never fetched."""
    return {
        "id": "a3s-code",
        "name": "A3S Code",
        "version": "acp-unattended",
        "description": "A3S Code ACP agent in unattended evaluation mode",
        "distribution": {
            "binary": {
                platform: {
                    "archive": "https://a3s.invalid/uploaded-by-adapter",
                    "cmd": COMMAND,
                    "args": [
                        "--unattended",
                        "--config",
                        REMOTE_CONFIG,
                        "--result-file",
                        RESULT_PATH,
                    ],
                    # The task container is the isolation boundary; this stays a
                    # separate opt-in from `--unattended` on purpose.
                    "env": {"A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX": "1"},
                }
            }
        },
    }


def must_raise(report: dict[str, Any] | None) -> bool:
    """A recorded attempt that used tools is scored; raising skips the verifier."""
    if not isinstance(report, dict):
        return True
    if report.get("outcome") == "succeeded":
        return False
    try:
        tool_calls = int(report.get("toolCalls") or 0)
    except (TypeError, ValueError):
        tool_calls = 0
    return tool_calls <= 0 or report.get("reason") == "provider_rejected"


class A3SCodeAcpAgent(AcpAgent):
    """Run `a3s-code-acp --unattended` through Harbor's ACP runner."""

    BINARY_ENV = "A3S_CODE_ACP_BINARY"
    CONFIG_ENV = "A3S_CODE_CONFIG"
    ENGINEERING_POLICY_ENV = "A3S_CODE_ACP_ENGINEERING_POLICY"
    EXECUTION_BUDGET_ENV = "A3S_CODE_TERMINAL_BENCH_MAX_EXECUTION_MS"
    GOAL_CONTINUATION_ENV = "A3S_CODE_ACP_GOAL_CONTINUATION"
    LANGUAGE_SERVERS_ENV = "A3S_CODE_ACP_LANGUAGE_SERVERS"
    LANGUAGE_SERVER_BUDGET_ENV = "A3S_CODE_ACP_LANGUAGE_SERVER_BUDGET_SEC"
    LSP_BUNDLE_ENV = "A3S_CODE_LSP_BUNDLE"
    PLATFORM_ENV = "A3S_CODE_ACP_PLATFORM"
    REASONING_EFFORT_ENV = "A3S_CODE_ACP_REASONING_EFFORT"
    STREAM_IDLE_ENV = "A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS"
    RUNNER_BUNDLE_ENV = "A3S_CODE_RUNNER_BUNDLE"
    SKILLS_ENV = "A3S_CODE_SKILLS_DIR"

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        extra_env = dict(kwargs.pop("extra_env", None) or {})
        platform = (
            extra_env.get(self.PLATFORM_ENV)
            or os.environ.get(self.PLATFORM_ENV)
            or "linux-x86_64"
        )
        kwargs.setdefault("registry_entry", registry_entry(platform))
        # Credentials ride the exec env, never the launcher script in the logs.
        forwarded = _forwarded_provider_env()
        if os.environ.get(self.STREAM_IDLE_ENV):
            forwarded[self.STREAM_IDLE_ENV] = os.environ[self.STREAM_IDLE_ENV]
        kwargs["extra_env"] = {**forwarded, **extra_env}
        super().__init__(*args, **kwargs)
        self._a3s_report: dict[str, Any] | None = None
        self._a3s_report_error: str | None = None

    def _local_file(self, key: str) -> Path:
        value = self._get_env(key)
        if not value:
            raise RuntimeError(f"{key} must point to a local file")
        path = Path(value).expanduser()
        if not path.is_file():
            raise FileNotFoundError(path)
        return path

    async def install(self, environment: BaseEnvironment) -> None:
        if self._get_env(self.RUNNER_BUNDLE_ENV):
            await self._unpack_runner_bundle(
                environment, self._local_file(self.RUNNER_BUNDLE_ENV)
            )
        await super().install(environment)

    async def _unpack_runner_bundle(
        self, environment: BaseEnvironment, bundle: Path
    ) -> None:
        """Local bytes from `prepare_runner_bundle.sh`; no network."""
        remote = "/tmp/a3s-acp-runner.tar.gz"
        venv = self._RUNNER_VENV_PATH
        try:
            await environment.upload_file(bundle, remote)
            await self.exec_as_root(
                environment,
                command=(
                    f"rm -rf {venv} && mkdir -p {venv} "
                    f"&& tar -xzf {remote} -C {venv} --no-same-owner "
                    f"&& rm -f {remote} && chmod -R a+rX {venv}"
                ),
                timeout_sec=DEFAULT_LANGUAGE_SERVER_BUDGET_SEC,
            )
        except Exception as error:  # noqa: BLE001 - Harbor's own install follows
            self.logger.warning(f"runner bundle skipped: {error}")

    def _build_dependencies_command(self, kind: Any) -> str:
        command = super()._build_dependencies_command(kind)
        if not self._get_env(self.RUNNER_BUNDLE_ENV):
            return command
        # A runner that imports ACP needs no apt or pip; anything else (musl,
        # a failed unpack) falls through to Harbor's install.
        probe = f'{self._RUNNER_VENV_PATH}/bin/python -c "import acp" >/dev/null 2>&1'
        return f"if {probe}; then exit 0; fi\n{command}"

    async def _install_binary_target(
        self, environment: BaseEnvironment, target: AcpBinaryTarget
    ) -> None:
        binary = self._local_file(self.BINARY_ENV)
        config = self._local_file(self.CONFIG_ENV)
        dist = f"{self._BINARY_INSTALL_DIR}/dist"
        remote_binary = f"{dist}/{COMMAND}"
        await self.exec_as_root(
            environment,
            command=f"rm -rf {self._BINARY_INSTALL_DIR} && mkdir -p {dist} /run/a3s",
        )
        await environment.upload_file(binary, remote_binary)
        await environment.upload_file(config, REMOTE_CONFIG)
        # Harbor may exec the agent as a non-root user; uploads land as root.
        await self.exec_as_root(
            environment,
            command=(
                f"chmod 755 {remote_binary} && chmod 644 {REMOTE_CONFIG} "
                f"&& chmod -R a+rX {self._BINARY_INSTALL_DIR}"
            ),
        )
        skills = self._get_env(self.SKILLS_ENV)
        if skills:
            skills_dir = Path(skills).expanduser()
            if not skills_dir.is_dir():
                raise NotADirectoryError(skills_dir)
            await environment.upload_dir(skills_dir, REMOTE_SKILLS)
            await self.exec_as_root(
                environment, command=f"chmod -R a+rX {REMOTE_SKILLS}"
            )
        if self._get_env(self.LANGUAGE_SERVERS_ENV) == "0":
            return
        if self._get_env(self.LSP_BUNDLE_ENV):
            bundle = self._local_file(self.LSP_BUNDLE_ENV)
            await self._unpack_language_server_bundle(environment, bundle)
        else:
            await self._install_language_servers(environment)

    async def _unpack_language_server_bundle(
        self, environment: BaseEnvironment, bundle: Path
    ) -> None:
        """Local bytes from `prepare_lsp_bundle.sh`; no network. A server the
        image already ships keeps its own `/usr/local/bin` entry."""
        remote = "/tmp/a3s-lsp.tar.gz"
        try:
            await environment.upload_file(bundle, remote)
            await self.exec_as_root(
                environment,
                command=(
                    f"mkdir -p {LSP_PREFIX} "
                    f"&& tar -xzf {remote} -C {LSP_PREFIX} --no-same-owner "
                    f"&& rm -f {remote} "
                    f'&& for f in {LSP_PREFIX}/bin/*; do '
                    f'[ -e "/usr/local/bin/${{f##*/}}" ] || ln -s "$f" /usr/local/bin/; '
                    "done"
                ),
                timeout_sec=DEFAULT_LANGUAGE_SERVER_BUDGET_SEC,
            )
        except Exception as error:  # noqa: BLE001 - never fail the trial on this
            self.logger.warning(f"language server bundle skipped: {error}")

    async def _install_language_servers(self, environment: BaseEnvironment) -> None:
        try:
            budget = int(
                self._get_env(self.LANGUAGE_SERVER_BUDGET_ENV)
                or DEFAULT_LANGUAGE_SERVER_BUDGET_SEC
            )
            script = language_server_setup(budget)
            # The slack covers apt's local install after the last download.
            await self.exec_as_root(
                environment,
                command=f"sh -c {shlex.quote(script)}",
                timeout_sec=budget + 60,
            )
        except Exception as error:  # noqa: BLE001 - never fail the trial on this
            self.logger.warning(f"language server setup skipped: {error}")

    def _build_launcher_script(self, kind: Any, target: Any) -> str:
        if isinstance(target, AcpBinaryTarget):
            args = list(target.args)
            for env, flag in (
                (self.EXECUTION_BUDGET_ENV, "--max-execution-ms"),
                (self.REASONING_EFFORT_ENV, "--reasoning-effort"),
                (self.ENGINEERING_POLICY_ENV, "--engineering-policy"),
            ):
                value = self._get_env(env)
                if value:
                    args += [flag, value]
            if self._get_env(self.GOAL_CONTINUATION_ENV) == "1":
                args.append("--goal-continuation")
            target = target.model_copy(update={"args": args})
        return super()._build_launcher_script(kind, target)

    async def run(
        self, instruction: str, environment: BaseEnvironment, context: AgentContext
    ) -> None:
        run_error: RuntimeError | None = None
        try:
            await super().run(instruction, environment, context)
        except RuntimeError as error:
            run_error = error
        result = await environment.exec(f"cat {shlex.quote(RESULT_PATH)}")
        if result.return_code == 0 and result.stdout:
            try:
                self._a3s_report = json.loads(result.stdout)
            except json.JSONDecodeError as error:
                self._a3s_report_error = f"invalid terminal result JSON: {error}"
        else:
            self._a3s_report_error = "terminal result file was not readable"
        if must_raise(self._a3s_report):
            reason = (self._a3s_report or {}).get("reason", "unknown")
            raise RuntimeError(
                f"a3s-code-acp did not finish a scorable attempt ({reason}); "
                "inspect acp-summary.json and a3s-code.result.json"
            ) from run_error

    def populate_context_post_run(self, context: AgentContext) -> None:
        super().populate_context_post_run(context)
        metadata = dict(context.metadata or {})
        metadata["result_file"] = RESULT_PATH
        report = self._a3s_report
        if report is not None:
            for key, field in (
                ("terminal_outcome", "outcome"),
                ("terminal_reason", "reason"),
                ("terminal_event", "terminalEvent"),
                ("duration_ms", "durationMs"),
                ("turns", "turns"),
                ("tool_calls", "toolCalls"),
                ("error_count", "errorCount"),
                ("provider", "provider"),
                ("provider_status", "providerStatus"),
            ):
                metadata[key] = report.get(field)
        elif self._a3s_report_error is not None:
            metadata["result_read_error"] = self._a3s_report_error
        context.metadata = metadata
