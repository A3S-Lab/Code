"""A Code gitlink still requires evidence and a gate touch in its parent landing."""

import contextlib
import importlib.util
import io
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "check_landing.py"
SPEC = importlib.util.spec_from_file_location("check_landing", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class LandingTests(unittest.TestCase):
    def run_landing(self, files, roadmap=False):
        evidence = "docs/evidence/code-harness/test/OUTCOME.md"

        def git(*args):
            if args == ("rev-parse", "--show-superproject-working-tree"):
                return "/a3s\n"
            if args[0] == "rev-list":
                return "candidate\n"
            if args[:3] == ("show", "-s", "--format=%h %s"):
                return "candidate Land Code\n"
            if args[:2] == ("show", "--name-status"):
                return "\n".join("M\t" + path for path in files)
            if args == ("show", "candidate:" + evidence):
                return "## Explicitly not claimed\nNo release.\n"
            if args[0] == "ls-files":
                return ""
            self.fail("unexpected git call: " + repr(args))

        stderr = io.StringIO()
        with patch.object(CHECK, "git", side_effect=git), \
             patch.object(CHECK, "load_allowlist", return_value=set()), \
             patch.object(CHECK, "submodule_roadmap_changed", return_value=roadmap), \
             patch.object(CHECK.os, "chdir") as chdir, \
             patch.object(CHECK.Path, "is_dir", return_value=True), \
             patch.object(CHECK.sys, "argv", [str(SCRIPT), "origin/main"]), \
             contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
            result = CHECK.main()
        chdir.assert_called_once_with("/a3s")
        return result, stderr.getvalue()

    def test_gitlink_is_first_party_code(self):
        self.assertTrue(CHECK.is_code("apps/code"))
        self.assertTrue(CHECK.is_code("apps/code/core/src/lib.rs"))
        self.assertFalse(CHECK.is_code("apps/code/vendor/ui/src/lib.rs"))
        self.assertFalse(CHECK.is_code("apps/studio"))

    def test_gitlink_without_same_commit_evidence_fails(self):
        result, stderr = self.run_landing(["apps/code"], roadmap=True)
        self.assertEqual(result, 1)
        self.assertIn("commits no docs/evidence/", stderr)

    def test_gitlink_without_gate_touch_fails(self):
        result, stderr = self.run_landing([
            "apps/code", "docs/evidence/code-harness/test/OUTCOME.md",
        ])
        self.assertEqual(result, 1)
        self.assertIn("no ROADMAP gate-row touch", stderr)

    def test_gitlink_with_evidence_and_pinned_roadmap_passes(self):
        result, stderr = self.run_landing([
            "apps/code", "docs/evidence/code-harness/test/OUTCOME.md",
        ], roadmap=True)
        self.assertEqual(result, 0, stderr)

    def test_pinned_roadmap_is_read_from_exact_gitlink_pair(self):
        with patch.object(CHECK, "git", side_effect=[
            ":160000 160000 before after M\tapps/code\n", "diff\n",
        ]) as git:
            self.assertTrue(CHECK.submodule_roadmap_changed("candidate"))
        self.assertEqual(git.call_args.args, (
            "-C", "apps/code", "diff", "before", "after", "--", "ROADMAP.md",
        ))

    def test_unchanged_pinned_roadmap_refuses(self):
        with patch.object(CHECK, "git", side_effect=[
            ":160000 160000 before after M\tapps/code\n", "",
        ]):
            self.assertFalse(CHECK.submodule_roadmap_changed("candidate"))


if __name__ == "__main__":
    unittest.main()
