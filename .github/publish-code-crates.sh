#!/usr/bin/env bash
# A release is complete only after both version-aligned crates are uploaded.
set -euo pipefail

test -n "${CARGO_REGISTRY_TOKEN:?CARGO_REGISTRY_TOKEN is required}"
version="$(python3 -c 'import tomllib; print(tomllib.load(open("core/Cargo.toml", "rb"))["package"]["version"])')"
bash scripts/check_release_versions.sh "$version"

for package in a3s-code-core a3s-code-acp; do
  response="$(mktemp)"
  status="$(curl --silent --show-error --location --output "$response" \
    --write-out '%{http_code}' \
    "https://crates.io/api/v1/crates/${package}/${version}")"
  case "$status" in
    200)
      python3 - "$response" "$package" "$version" <<'PY'
import json
import sys

with open(sys.argv[1]) as source:
    version = json.load(source)["version"]
if (version["crate"], version["num"], version["yanked"]) != (sys.argv[2], sys.argv[3], False):
    raise SystemExit("registry version does not match the active release")
PY
      rm -f "$response"
      echo "$package $version already published"
      ;;
    404)
      rm -f "$response"
      cargo publish --locked --allow-dirty -p "$package"
      ;;
    *)
      rm -f "$response"
      echo "Cannot verify $package $version: registry HTTP $status" >&2
      exit 1
      ;;
  esac
done
