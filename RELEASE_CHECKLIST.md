# Release Checklist for a3s-code v1.3.4

## Pre-Release Checks

### 1. Code Changes
- [x] Bug fix: Permission wildcard matching for MCP tools
- [x] Added tests: 2 new unit tests, 5 integration tests
- [x] All tests passing: 65/65 ✅

### 2. Version Alignment
Current: 1.3.3 → Target: 1.3.4

Files to update:
- [ ] `core/Cargo.toml`
- [ ] `sdk/node/Cargo.toml`
- [ ] `sdk/node/package.json`
- [ ] `sdk/python/Cargo.toml`
- [ ] `sdk/python/pyproject.toml`
- [ ] `Cargo.lock` (via `cargo check`)

### 3. Testing
- [x] Unit tests: `cargo test --lib permissions::tests` (50/50)
- [x] Integration tests: `cargo test --test test_subagent_permissions` (5/5)
- [x] Scenario tests: `cargo run --example test_scenario1` (10/10)
- [ ] Format check: `cargo fmt --all -- --check`
- [ ] Clippy: `cargo clippy --workspace -- -D warnings`

### 4. Documentation
- [x] TEST_RESULT.md created
- [x] TEST_REPORT.md created
- [ ] CHANGELOG.md updated (if exists)
- [ ] README.md updated (if needed)

### 5. Git Operations
- [ ] Commit changes with descriptive message
- [ ] Create tag: `v1.3.4`
- [ ] Push to origin: `git push origin main`
- [ ] Push tag: `git push origin v1.3.4`

### 6. GitHub Actions
After pushing tag, verify:
- [ ] CI checks pass (Linux)
- [ ] CI checks pass (Windows)
- [ ] Publish to crates.io succeeds
- [ ] Node SDK publish succeeds
- [ ] Python SDK publish succeeds
- [ ] GitHub Release created

## Release Commands

### Option 1: Automated (Recommended)
```bash
cd /Users/roylin/Desktop/code/a3s/crates/code
./release.sh 1.3.4
```

### Option 2: Manual
```bash
cd /Users/roylin/Desktop/code/a3s/crates/code

# 1. Check version alignment
./check-version.sh

# 2. Update versions manually
# Edit: core/Cargo.toml, sdk/*/Cargo.toml, sdk/node/package.json, sdk/python/pyproject.toml

# 3. Update Cargo.lock
cargo check --workspace

# 4. Run tests
cargo test --lib permissions::tests
cargo test --test test_subagent_permissions

# 5. Format code
cargo fmt --all

# 6. Commit and tag
git add -A
git commit -m "chore: bump version to 1.3.4"
git tag -a v1.3.4 -m "Release v1.3.4"

# 7. Push
git push origin main
git push origin v1.3.4
```

## Release Notes (v1.3.4)

### 🐛 Bug Fixes
- **Permission System**: Fix wildcard matching for MCP tool names
  - `mcp__longvt__*` now correctly matches all longvt tools
  - `mcp__*` now correctly matches all MCP tools
  - `permissive_deny` parameter now works as expected
  - Agent .md deny rules now work with permissive mode

### ✨ Features
- Add wildcard support to `matches_tool_name` function
- Support `*` and `?` wildcards in tool name patterns

### 🧪 Tests
- Add 2 new unit tests for wildcard matching
- Add 5 integration tests for SubAgent permissions
- Add scenario test program
- All 65 tests passing

### 📝 Documentation
- Add comprehensive test report
- Add permission control demo program
- Add test agent definition example

### 🔧 Technical Details
- Modified: `core/src/permissions.rs:134-150`
- Added: `core/tests/test_subagent_permissions.rs`
- Added: `core/examples/permission_control.rs`
- Added: `core/examples/test_scenario1.rs`

## Post-Release Verification

After release completes:
1. [ ] Check crates.io: https://crates.io/crates/a3s-code-core
2. [ ] Check npm: https://www.npmjs.com/package/@a3s-lab/code
3. [ ] Check PyPI: https://pypi.org/project/a3s-code/
4. [ ] Check GitHub Release: https://github.com/A3S-Lab/Code/releases/tag/v1.3.4
5. [ ] Test installation:
   ```bash
   # Rust
   cargo add a3s-code-core@1.3.4

   # Node
   npm install @a3s-lab/code@1.3.4

   # Python
   pip install a3s-code==1.3.4
   ```

## Rollback Plan

If release fails:
```bash
# Delete tag locally
git tag -d v1.3.4

# Delete tag remotely
git push origin :refs/tags/v1.3.4

# Revert commit
git revert HEAD
git push origin main
```

## Notes
- This is a bug fix release (patch version bump)
- No breaking changes
- Backward compatible with 1.3.x
- All existing tests continue to pass
