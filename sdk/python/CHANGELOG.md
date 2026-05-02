# Changelog

All notable changes to the A3S Code Python SDK will be documented in this file.

## [2.1.0] - Unreleased

### Added
- Added explicit `planning_mode="auto" | "enabled" | "disabled"` while keeping
  the legacy `planning` boolean shortcut.
- Added `delegate_task(...)`, `parallel_task(...)`, and `tool_definitions()` to
  mirror the Rust core and Node SDK surfaces.
- Aligned AHP-backed session hooks with AHP 2.4.

## [2.0.0] - 2026-05-02

### Changed
- Reframed the primary SDK path around `Agent` and workspace-bound sessions.
- Made `task` / `parallel_task` the single recommended multi-agent delegation
  path for new integrations.
- Removed the standalone 1.x lifecycle control-plane API from the 2.0 SDK
  surface; routine delegation uses `task` / `parallel_task`.
- Clarified that lane queues are optional external/hybrid dispatch
  infrastructure, not a generic task submission API.

### Removed
- Removed 1.0-era public task/progress/idle lifecycle bindings from the SDK.
- Removed generic session queue submission helpers from the public SDK surface.
- Removed duplicate team shortcuts, lifecycle control bindings, and
  unified slot APIs.
- Removed explicit Lead/Worker/Reviewer team-runner APIs.
- Removed placeholder delegated-agent execution and standalone child-run handle/event
  bindings.

## [1.4.5] - 2026-03-16

Historical note: this section records pre-2.0 investigation work.

### Added
- Support for `kind: tool` in skill system (in addition to `instruction` and `persona`)
- Complete PyO3 bindings for pre-2.0 delegated-agent configuration fields
- Comprehensive integration tests with real LLM execution

### Fixed
- Delegated-agent `skill_dirs` parameter now properly passed from Python to Rust layer
- Skill registry now correctly filters and loads tool-type skills
- All delegated-agent configuration fields now have proper getter/setter methods

### Changed
- Reorganized test structure: `tests/unit/` for mock tests, `tests/integration/` for real API tests
- Removed hardcoded API keys from test files (now use environment variables)

### Security
- All test files now use environment variables for API credentials
- Added `.gitignore` to prevent accidental credential commits

## [1.4.4] - 2026-03-15

Historical note: this was an early Python SDK release before the 2.0 API
boundary.

### Added
- Initial Python SDK release with PyO3 bindings
- Basic agent and session management
- Skill system support
- Early lifecycle control-plane and delegated-agent execution support
