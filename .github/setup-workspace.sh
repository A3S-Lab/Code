#!/bin/bash
# Setup for building the Code workspace standalone in CI.
# Replaces path dependencies with crates.io versions so we don't need
# the full monorepo.

set -euo pipefail

echo "Replacing path dependencies with crates.io versions..."

# core/Cargo.toml — internal crate deps
sed -i.bak \
  -e 's|a3s-tools-core = { version = "0.1", path = "../../tools-core" }|a3s-tools-core = "0.1"|' \
  -e 's|a3s-lane = { version = "0.1", path = "../../lane" }|a3s-lane = "0.1"|' \
  -e 's|a3s-cron = { version = "0.1", path = "../../cron" }|a3s-cron = "0.1"|' \
  -e 's|a3s-privacy = { version = "0.1", path = "../../privacy" }|a3s-privacy = "0.1"|' \
  core/Cargo.toml
rm -f core/Cargo.toml.bak

# server/Cargo.toml — internal crate deps
sed -i.bak \
  -e 's|a3s-cron = { version = "0.1", path = "../../cron" }|a3s-cron = "0.1"|' \
  -e 's|a3s-updater = { version = "0.2", path = "../../updater" }|a3s-updater = "0.2"|' \
  server/Cargo.toml
rm -f server/Cargo.toml.bak

echo "Path dependencies replaced. Ready to build."
