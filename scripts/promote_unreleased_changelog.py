#!/usr/bin/env python3
"""Move Keep-a-Changelog Unreleased notes under a version heading.

Does not commit, tag, or push. Release automation calls this after a version
bump and before version consistency checks. An empty Unreleased section is a
no-op. Existing version headings are not rewritten.
"""

import datetime
import sys
from pathlib import Path


def promote(text, version, released_on):
    lines = text.splitlines(keepends=True)
    start = None
    for index, line in enumerate(lines):
        if line.startswith("## Unreleased"):
            start = index
            break
    if start is None:
        raise SystemExit("changelog has no ## Unreleased section")

    end = len(lines)
    for index in range(start + 1, len(lines)):
        if lines[index].startswith("## "):
            end = index
            break

    body = lines[start + 1 : end]
    if not any(line.startswith("- ") for line in body):
        return text

    heading = f"## [{version}]"
    if any(line.startswith(heading) for line in lines):
        raise SystemExit(f"changelog already has {heading}")

    promoted = [f"{heading} - {released_on}\n", "\n", *body]
    if promoted[-1] and not promoted[-1].endswith("\n"):
        promoted[-1] += "\n"
    if end < len(lines) and not promoted[-1].endswith("\n\n"):
        promoted.append("\n")

    rewritten = lines[: start + 1] + ["\n"] + promoted + lines[end:]
    return "".join(rewritten)


def main():
    if len(sys.argv) < 3:
        raise SystemExit(
            "usage: promote_unreleased_changelog.py <version> <changelog>..."
        )
    version = sys.argv[1]
    released_on = datetime.date.today().isoformat()
    for raw_path in sys.argv[2:]:
        path = Path(raw_path)
        original = path.read_text(encoding="utf-8")
        updated = promote(original, version, released_on)
        if updated == original:
            print(f"no Unreleased notes in {path}")
            continue
        path.write_text(updated, encoding="utf-8")
        print(f"promoted Unreleased notes in {path} under {version}")


if __name__ == "__main__":
    main()
