#!/bin/bash
# Setup for building the Code workspace standalone in CI.
# Replaces path dependencies with crates.io versions so we don't need
# the full monorepo.

set -euo pipefail

echo "Replacing path dependencies with crates.io versions..."

# core/Cargo.toml — internal crate deps
sed -i.bak \
  -e 's|a3s-common = { version = "0.1", path = "../../common" }|a3s-common = "0.1.1"|' \
  -e 's|a3s-memory = { version = "0.1.1", path = "../../memory" }|a3s-memory = "0.1.1"|' \
  -e 's|a3s-lane = { version = "0.4", path = "../../lane" }|a3s-lane = "0.4"|' \
  -e 's|a3s-search = { version = "0.8", path = "../../search", default-features = false }|a3s-search = { version = "0.8", default-features = false }|' \
  -e 's|a3s-box-sdk = { version = "0.7", path = "../../box/src/sdk", optional = true }|a3s-box-sdk = { version = "0.7", optional = true }|' \
  core/Cargo.toml
rm -f core/Cargo.toml.bak

echo "Path dependencies replaced. Ready to build."
