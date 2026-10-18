"""The native Linux lane must report a failed test binary as a failed lane."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[2] / "scripts/linux_arm64_lane.sh"


class LinuxLaneTests(unittest.TestCase):
    def test_test_binary_failure_is_not_hidden_by_log_collection(self):
        for test_exit in (0, 7):
            with self.subTest(test_exit=test_exit), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                script = root / "apps/code/scripts/linux_arm64_lane.sh"
                script.parent.mkdir(parents=True)
                shutil.copyfile(SCRIPT, script)
                commands = root / "bin"
                commands.mkdir()
                rustup = commands / "rustup"
                rustup.write_text('#!/bin/sh\nprintf \'%s\\n\' "$@" > "$A3S_TEST_ARGS_FILE"\nprintf \'%s\\n\' \'{"reason":"compiler-artifact","executable":"/fixture/core-tests","target":{"name":"a3s_code_core","kind":["lib"]}}\'\n')
                docker = commands / "docker"
                docker.write_text('''#!/bin/sh
case "$1" in
  info) echo aarch64 ;;
  build) cat >/dev/null ;;
  run) exit "$A3S_TEST_RUN_EXIT" ;;
  *) exit 2 ;;
esac
''')
                rustup.chmod(0o755)
                docker.chmod(0o755)
                logs = root / "logs"
                result = subprocess.run(
                    ["bash", str(script), "lib", str(logs)], capture_output=True,
                    text=True, timeout=10, env={
                        **os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"],
                        "A3S_TEST_RUN_EXIT": str(test_exit),
                        "A3S_TEST_ARGS_FILE": str(root / "build-args"),
                    },
                )
                self.assertIn(f"lib.log exit={test_exit}", (logs / "summary.txt").read_text())
                self.assertEqual(result.returncode, 0 if test_exit == 0 else 1, result.stdout + result.stderr)
                build_args = (root / "build-args").read_text().splitlines()
                self.assertIn("--locked", build_args)
                self.assertIn("--lib", build_args)
                self.assertNotIn("--tests", build_args)


if __name__ == "__main__":
    unittest.main()
