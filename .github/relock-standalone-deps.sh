#!/bin/bash
# Resolve the manifests' actual pinned sources after SDK path patches are removed.
set -euo pipefail

for manifest in Cargo.toml sdk/node/Cargo.toml sdk/python/Cargo.toml; do
  cargo metadata --manifest-path "$manifest" --format-version 1 --all-features >/dev/null
  cargo metadata --manifest-path "$manifest" --format-version 1 --all-features --locked >/dev/null
done
