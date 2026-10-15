#!/usr/bin/env python3
"""Qualify the actual POSIX pager binary's non-terminal startup contract."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def run_case(binary: Path, root: Path, name: str, arguments: list[str]) -> dict:
    case = root / name
    case.mkdir()
    product_home = case / "product-home"
    product_home.mkdir()
    workspace = case / "workspace"
    workspace.mkdir()
    environment = {
        "PATH": os.environ.get("PATH", os.defpath),
        "TERM": "dumb",
        "GROK_HOME": str(product_home),
        "A3S_HOME": str(product_home),
    }
    command = [str(binary), "--no-auto-update", "--no-leader", *arguments]
    with (case / "stdout").open("wb") as output, (case / "stderr").open("wb") as errors:
        child = subprocess.Popen(
            command,
            cwd=workspace,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=errors,
            start_new_session=True,
        )
        timed_out = False
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
    with (case / "stdout").open("rb") as output:
        stdout = output.read(4096).decode("utf-8", errors="replace")
    with (case / "stderr").open("rb") as errors:
        stderr = errors.read(4096).decode("utf-8", errors="replace")
    return {
        "case": name,
        "exit": child.returncode,
        "timeout": timed_out,
        "stdout": stdout,
        "stderr": stderr,
        "stdout_bytes": (case / "stdout").stat().st_size,
        "stderr_bytes": (case / "stderr").stat().st_size,
        "startup_files": [
            str(path.relative_to(case))
            for directory in (product_home, workspace)
            for path in directory.rglob("*")
            if path.is_file()
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--expected-revision")
    options = parser.parse_args()
    if os.name != "posix":
        parser.error("this gate owns POSIX process groups; Windows needs its native gate")
    if options.expected_revision and (
        len(options.expected_revision) != 40
        or any(character not in "0123456789abcdef" for character in options.expected_revision)
    ):
        parser.error("expected revision must be the full lowercase Git commit id")
    binary = options.binary.resolve(strict=True)
    if not binary.is_file():
        parser.error("binary must be a regular file")
    original_digest = digest(binary)
    results = []
    with tempfile.TemporaryDirectory(prefix="a3s-code-non-tty-") as temporary:
        root = Path(temporary)
        cases = [
            ("default", []),
            ("explicit-model", ["--model", "test/no-network", "--cwd", str(root / "absent")]),
            ("resume", ["--resume", "saved-session", "--model", "test/no-network"]),
            ("dashboard", ["dashboard"]),
        ]
        for name, arguments in cases:
            result = run_case(binary, root, name, arguments)
            expected_prefix = f"A3S Code {options.expected_version} ("
            lines = result["stdout"].splitlines()
            header = lines[0] if lines else ""
            valid = (
                result["exit"] == 0
                and not result["timeout"]
                and result["stderr_bytes"] == 0
                and result["stdout_bytes"] < 4096
                and header.startswith(expected_prefix)
                and header.endswith(")")
                and len(lines) == 3
                and result["stdout"].endswith("Interactive TUI requires terminal input and output.\n")
                and not result["startup_files"]
            )
            if name in ("explicit-model", "resume"):
                valid = valid and "\nmodel: test/no-network\n" in result["stdout"]
            if options.expected_revision:
                valid = valid and header == (
                    f"A3S Code {options.expected_version} ({options.expected_revision[:12]})"
                )
            result["passed"] = valid
            results.append(result)
    stable = digest(binary) == original_digest
    passed = stable and all(result["passed"] for result in results)
    print(json.dumps({
        "passed": passed,
        "binary": str(binary),
        "binary_sha256": original_digest,
        "binary_stable": stable,
        "cases": results,
    }, indent=2))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
