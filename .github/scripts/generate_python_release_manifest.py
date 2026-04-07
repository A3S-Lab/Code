#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input_dir")
    parser.add_argument("output_file")
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    input_dir = Path(args.input_dir)
    output_file = Path(args.output_file)
    wheels = sorted(input_dir.glob("*.whl"))
    manifest = {
        "version": args.version,
        "assets": [{"filename": wheel.name, "sha256": sha256(wheel)} for wheel in wheels],
    }
    output_file.parent.mkdir(parents=True, exist_ok=True)
    output_file.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
