from __future__ import annotations

import json
import os
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, ExecInput
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


RESULT_BEGIN = "A3S_CODE_RESULT_BEGIN"
RESULT_END = "A3S_CODE_RESULT_END"
REMOTE_SKILLS_DIR = "/workspace/.skillsbench-skills"


class A3SCode(BaseInstalledAgent):
    """Harbor installed agent wrapper for a3s-code."""

    SUPPORTS_ATIF = False

    def __init__(self, *args, **kwargs):
        version = kwargs.pop("version", None)
        if version is None:
            version = os.getenv("A3S_CODE_VERSION")
        super().__init__(*args, version=version, **kwargs)

    @staticmethod
    def _default_env() -> dict[str, str]:
        return {
            "A3S_CODE_CONFIG": os.getenv("A3S_CODE_CONFIG", "/workspace/.a3s/config.hcl"),
            "A3S_CODE_WORKSPACE": os.getenv("A3S_CODE_WORKSPACE", "/workspace"),
            "A3S_CODE_BUILTIN_SKILLS": os.getenv("A3S_CODE_BUILTIN_SKILLS", "true"),
            "A3S_CODE_PLANNING": os.getenv("A3S_CODE_PLANNING", "true"),
            "A3S_CODE_PERMISSIVE": os.getenv("A3S_CODE_PERMISSIVE", "true"),
        }

    @staticmethod
    def name() -> str:
        return "a3s-code"

    def version(self) -> str | None:
        value = super().version()
        if value:
            return value
        return os.getenv("A3S_CODE_VERSION")

    @property
    def _install_agent_template_path(self) -> Path:
        return Path(__file__).with_name("install_agent.sh.j2")

    @property
    def _template_variables(self) -> dict[str, str]:
        version = self.version()
        version_spec = f"=={version}" if version else ""
        runner_source = Path(__file__).with_name("a3s_code_runner.py").read_text()
        return {
            "version_spec": version_spec,
            "runner_source": runner_source,
        }

    def _setup_env(self) -> dict[str, str]:
        env = super()._setup_env()
        env.update(self._default_env())
        for name in ("A3S_CODE_PIP_SPEC", "A3S_CODE_VERSION"):
            value = os.getenv(name)
            if value:
                env[name] = value
        return env

    @staticmethod
    def _jsonable(value):
        if value is None:
            return None
        if isinstance(value, (str, int, float, bool)):
            return value
        if isinstance(value, dict):
            return {str(k): A3SCode._jsonable(v) for k, v in value.items()}
        if isinstance(value, (list, tuple)):
            return [A3SCode._jsonable(v) for v in value]
        if hasattr(value, "model_dump"):
            return A3SCode._jsonable(value.model_dump())
        if hasattr(value, "dict"):
            return A3SCode._jsonable(value.dict())
        if hasattr(value, "__dict__"):
            return A3SCode._jsonable(vars(value))
        return str(value)

    def get_version_command(self) -> str | None:
        return "/opt/a3s-code-venv/bin/python -c 'import a3s_code; print(getattr(a3s_code, \"__version__\", \"unknown\"))'"

    async def _upload_skills_dir(self, environment: BaseEnvironment, source_dir: Path) -> str:
        await environment.exec(command=f"mkdir -p {REMOTE_SKILLS_DIR}")
        for path in sorted(source_dir.rglob("*")):
            if not path.is_file():
                continue
            relative = path.relative_to(source_dir).as_posix()
            target_path = f"{REMOTE_SKILLS_DIR}/{relative}"
            parent = str(Path(target_path).parent).replace(" ", "\\ ")
            await environment.exec(command=f"mkdir -p {parent}")
            await environment.upload_file(source_path=path, target_path=target_path)
        return REMOTE_SKILLS_DIR

    async def setup(self, environment: BaseEnvironment) -> None:
        await super().setup(environment)
        skills_dir = getattr(self, "skills_dir", None)
        if not skills_dir:
            return

        source_dir = Path(skills_dir)
        upload_dir = self.logs_dir / "skills-upload"
        upload_dir.mkdir(parents=True, exist_ok=True)

        if not source_dir.exists():
            (upload_dir / "status.txt").write_text(
                f"skills_dir configured but not found locally: {source_dir}\n"
            )
            return

        remote_dir = await self._upload_skills_dir(environment, source_dir)
        (upload_dir / "status.txt").write_text(
            f"uploaded {source_dir} -> {remote_dir}\n"
        )

    def create_run_agent_commands(self, instruction: str) -> list[ExecInput]:
        env = self._default_env()
        env["A3S_CODE_INSTRUCTION"] = instruction
        skills_dir = getattr(self, "skills_dir", None)
        if skills_dir:
            remote_skill_dir = REMOTE_SKILLS_DIR
            if Path(skills_dir).exists():
                env["A3S_CODE_SKILL_DIRS_JSON"] = json.dumps([remote_skill_dir])
            else:
                env["A3S_CODE_SKILL_DIRS_JSON"] = json.dumps([str(skills_dir)])
        mcp_servers = getattr(self, "mcp_servers", None)
        if mcp_servers:
            env["A3S_CODE_MCP_SERVERS_JSON"] = json.dumps(self._jsonable(mcp_servers))
        for key, value in list(env.items()):
            if isinstance(value, bool):
                env[key] = "true" if value else "false"
            elif value is None:
                del env[key]
            else:
                env[key] = str(value)
        command = (
            ". /opt/a3s-code-venv/bin/activate && "
            "python /installed-agent/a3s_code_runner.py"
        )
        return [
            ExecInput(
                command=command,
                cwd=env.get("A3S_CODE_WORKSPACE", "/workspace"),
                env=env,
                timeout_sec=None,
            )
        ]

    def populate_context_post_run(self, context: AgentContext) -> None:
        stdout_path = self.logs_dir / "command-0" / "stdout.txt"
        metadata = {
            "agent": self.name(),
            "version": self.version(),
        }
        if not stdout_path.exists():
            context.metadata = metadata
            return

        stdout = stdout_path.read_text()
        start = stdout.find(RESULT_BEGIN)
        end = stdout.find(RESULT_END)
        if start == -1 or end == -1 or end <= start:
            metadata["raw_stdout"] = stdout
            context.metadata = metadata
            return

        payload_text = stdout[start + len(RESULT_BEGIN):end].strip()
        try:
            payload = json.loads(payload_text)
        except json.JSONDecodeError:
            metadata["raw_stdout"] = stdout
            context.metadata = metadata
            return

        context.n_input_tokens = payload.get("prompt_tokens")
        context.n_output_tokens = payload.get("completion_tokens")
        metadata.update(payload)
        context.metadata = metadata
