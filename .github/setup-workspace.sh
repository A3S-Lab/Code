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
  -e 's|a3s-search = { version = "1.0", path = "../../search", default-features = false }|a3s-search = { version = "1.0", default-features = false }|' \
  -e 's|a3s-box-sdk = { version = "0.7", path = "../../box/src/sdk", optional = true }|a3s-box-sdk = { version = "0.7", optional = true }|' \
  -e 's|a3s-ahp = { version = "2.0", path = "../../ahp", optional = true, features = \["http", "websocket", "unix-socket"\] }|a3s-ahp = { version = "2.0", optional = true, features = ["http", "websocket", "unix-socket"] }|' \
  core/Cargo.toml
rm -f core/Cargo.toml.bak

# sdk/python/Cargo.toml — remove a3s-ahp dependency (not published to crates.io yet)
sed -i.bak \
  -e '/a3s-ahp = { version = "0.1", path = "\.\.\/\.\.\/\.\.\/ahp" }/d' \
  sdk/python/Cargo.toml
rm -f sdk/python/Cargo.toml.bak

# sdk/node/Cargo.toml — remove a3s-ahp dependency (not published to crates.io yet)
sed -i.bak \
  -e '/a3s-ahp = { version = "0.1", path = "\.\.\/\.\.\/\.\.\/ahp" }/d' \
  sdk/node/Cargo.toml
rm -f sdk/node/Cargo.toml.bak

# core/Cargo.toml — replace ahp path dep in dev-dependencies
sed -i.bak \
  -e 's|a3s-ahp = { path = "../../ahp" }|a3s-ahp = "2.0"|' \
  core/Cargo.toml
rm -f core/Cargo.toml.bak

echo "Path dependencies replaced. Ready to build."
