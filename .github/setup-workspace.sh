#!/bin/bash
# Setup for building the Code crate standalone in CI.
# Replaces path dependencies with crates.io versions so we don't need
# the full monorepo workspace.

set -euo pipefail

echo "Replacing path dependencies with crates.io versions..."

# Replace path deps with registry versions in Cargo.toml
sed -i.bak \
  -e 's|a3s-tools-core = { version = "0.1", path = "../tools-core" }|a3s-tools-core = "0.1"|' \
  -e 's|a3s-lane = { version = "0.1", path = "../lane" }|a3s-lane = "0.1"|' \
  -e 's|a3s-cron = { version = "0.1", path = "../cron" }|a3s-cron = "0.1"|' \
  -e 's|a3s-privacy = { version = "0.1", path = "../privacy" }|a3s-privacy = "0.1"|' \
  -e 's|a3s-updater = { version = "0.2", path = "../updater" }|a3s-updater = "0.2"|' \
  Cargo.toml

rm -f Cargo.toml.bak

echo "Path dependencies replaced. Ready to build."
