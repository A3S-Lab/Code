#!/bin/bash
# Setup for building the Code workspace standalone in CI.
# Replaces path dependencies with git dependencies so we don't need
# the full monorepo checked out.

set -euo pipefail

echo "Replacing path dependencies with git dependencies..."

# core/Cargo.toml — internal crate deps
sed -i.bak \
  -e 's|a3s-tools-core = { version = "0.1", path = "../../tools-core" }|a3s-tools-core = { git = "https://github.com/A3S-Lab/a3s.git", branch = "main" }|' \
  -e 's|a3s-lane = { version = "0.1", path = "../../lane" }|a3s-lane = { git = "https://github.com/A3S-Lab/Lane.git" }|' \
  -e 's|a3s-cron = { version = "0.1", path = "../../cron" }|a3s-cron = { git = "https://github.com/A3S-Lab/Cron.git" }|' \
  -e 's|a3s-privacy = { version = "0.1", path = "../../privacy" }|a3s-privacy = { git = "https://github.com/A3S-Lab/a3s.git", branch = "main" }|' \
  core/Cargo.toml
rm -f core/Cargo.toml.bak

# server/Cargo.toml — internal crate deps
sed -i.bak \
  -e 's|a3s-cron = { version = "0.1", path = "../../cron" }|a3s-cron = { git = "https://github.com/A3S-Lab/Cron.git" }|' \
  -e 's|a3s-updater = { version = "0.2", path = "../../updater" }|a3s-updater = { git = "https://github.com/A3S-Lab/Updater.git" }|' \
  server/Cargo.toml
rm -f server/Cargo.toml.bak

echo "Path dependencies replaced with git deps. Ready to build."
