#!/usr/bin/env bash
# Check version alignment across release artifacts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

scripts/check_release_versions.sh
