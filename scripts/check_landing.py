#!/usr/bin/env python3
"""Landing-discipline ratchet for apps/code (constitution II + standards §10).

Every code landing commits together with its evidence OUTCOME.md and a
ROADMAP gate-row touch, in one commit. Every OUTCOME.md carries the
"Explicitly not claimed" section; the pre-ratchet files that use renamed
equivalents are grandfathered in a shrink-only allowlist.

Checks, for <base>..HEAD (merges skipped):
  1. a commit touching first-party code must also touch
     docs/evidence/**/OUTCOME.md (the evidence rule), and
  2. must also touch apps/code/ROADMAP.md or a docs/*roadmap* file
     (the gate-row rule), and
  3. every OUTCOME.md the commit adds or modifies must carry the exact
     "Explicitly not claimed" heading unless it is allowlisted, and
  4. the allowlist only shrinks: an entry whose file is gone, or whose
     file now carries the heading, must be removed from the list.

Usage: check_landing.py <base_ref>
"""

from __future__ import annotations

import re
import os
import subprocess
import sys
from pathlib import Path, PurePosixPath

HEADING = re.compile(r"^#{1,6}\s+Explicitly not claimed\s*$", re.MULTILINE)
CODE_EXTS = {".rs", ".toml", ".lock", ".py", ".mjs", ".sh", ".js", ".ts", ".acl", ".go"}
CODE_PREFIX = "apps/code/"
CODE_EXCLUDED_PARTS = ("vendor", "website", "target", "fixtures")
ALLOWLIST_PATH = PurePosixPath("apps/code/scripts/check_landing_allowlist.txt")


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def is_code(path: str) -> bool:
    if path == CODE_PREFIX.rstrip("/"):
        return True
    if not path.startswith(CODE_PREFIX):
        return False
    rest = PurePosixPath(path[len(CODE_PREFIX):])
    if any(part in CODE_EXCLUDED_PARTS for part in rest.parts):
        return False
    return rest.suffix in CODE_EXTS


def is_evidence(path: str) -> bool:
    return path.startswith("docs/evidence/") and path.endswith("/OUTCOME.md")


def is_roadmap(path: str) -> bool:
    if path == "apps/code/ROADMAP.md":
        return True
    return path.startswith("docs/") and "roadmap" in PurePosixPath(path).name


def submodule_roadmap_changed(revision: str) -> bool:
    """A gitlink landing includes the ROADMAP change in its pinned Code tree."""
    path = CODE_PREFIX.rstrip("/")
    changes = git("diff-tree", "--no-commit-id", "--raw", "-r", revision, "--", path)
    for line in changes.splitlines():
        fields = line.split()
        if len(fields) != 6 or fields[-1] != path:
            continue
        old_mode, new_mode, before, after, _, _ = fields
        if old_mode != ":160000" or new_mode != "160000":
            return False
        return bool(git("-C", path, "diff", before, after, "--", "ROADMAP.md").strip())
    return False


def commit_files(revision: str) -> list[str]:
    """Added/modified/copy/rename targets; deletions are excluded (an
    OUTCOME.md is an evidence anchor and is never deleted, but a code
    file deletion is still a landing and must not crash the checker)."""
    out = git("show", "--name-status", "--format=", "-r", revision)
    kept: list[str] = []
    for line in out.splitlines():
        if not line.strip():
            continue
        status, _, path = line.partition("\t")
        if status.startswith("D"):
            continue
        kept.append(path.strip())
        if "\t" in path:  # rename/copy "old\tnew" — the new path counts
            kept[-1] = path.split("\t")[-1].strip()
    return kept


def load_allowlist() -> set[str]:
    # Read the working-tree file first so the checker is runnable before
    # its own first commit; the committed blob is the fallback.
    worktree = Path(ALLOWLIST_PATH.as_posix())
    if worktree.exists():
        text = worktree.read_text()
    else:
        text = git("show", f"HEAD:{ALLOWLIST_PATH.as_posix()}")
    return {
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.startswith("#")
    }


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    # Root evidence and the Code gitlink land together in the superproject.
    root = git("rev-parse", "--show-superproject-working-tree").strip()
    if not root:
        root = git("rev-parse", "--show-toplevel").strip()
    os.chdir(root)
    if not Path(CODE_PREFIX).is_dir():
        print("landing evidence requires the A3S OS repository with apps/code", file=sys.stderr)
        return 1
    base = sys.argv[1]
    rev_list = git("rev-list", "--no-merges", f"{base}..HEAD").split()
    if not rev_list:
        print(f"no non-merge commits in {base}..HEAD")
        return 0

    allowlist = load_allowlist()
    failures: list[str] = []

    for revision in reversed(rev_list):  # oldest first, like the log
        subject = git("show", "-s", "--format=%h %s", revision).strip()
        files = commit_files(revision)
        code = [f for f in files if is_code(f)]
        evidence = [f for f in files if is_evidence(f)]

        if code:
            if not evidence:
                failures.append(
                    f"{subject}: touches first-party code but commits no "
                    f"docs/evidence/**/OUTCOME.md — standards §10: code + "
                    f"tests + evidence + ROADMAP row land in ONE commit"
                )
            roadmap_changed = any(is_roadmap(f) for f in files)
            if not roadmap_changed and CODE_PREFIX.rstrip("/") in code:
                roadmap_changed = submodule_roadmap_changed(revision)
            if not roadmap_changed:
                failures.append(
                    f"{subject}: no ROADMAP gate-row touch "
                    f"(apps/code/ROADMAP.md or docs/*roadmap*)"
                )

        for path in evidence:
            blob = git("show", f"{revision}:{path}")
            if HEADING.search(blob):
                continue
            if path in allowlist:
                continue
            failures.append(
                f"{subject}: {path} lacks the mandatory "
                f'"Explicitly not claimed" section'
            )

    # Shrink-only: the allowlist may not carry stale entries.
    tracked_outcomes = git("ls-files", "docs/evidence/**/OUTCOME.md").split()
    for entry in sorted(allowlist):
        if entry not in tracked_outcomes:
            failures.append(
                f"allowlist entry {entry} no longer exists — shrink the list"
            )
        else:
            blob = git("show", f"HEAD:{entry}")
            if HEADING.search(blob):
                failures.append(
                    f"allowlist entry {entry} now carries the heading — "
                    f"shrink the list"
                )

    if failures:
        print("landing discipline violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print(
        f"landing discipline clean: {len(rev_list)} non-merge commits "
        f"checked against base {base}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
