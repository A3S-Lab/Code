# Changelog

All notable changes to the A3S Code Python SDK will be documented in this file.

## [1.4.5] - 2026-03-16

### Added
- Support for `kind: tool` in skill system (in addition to `instruction` and `persona`)
- Complete PyO3 bindings for `SubAgentConfig` with all 12 parameters exposed
- Comprehensive integration tests with real LLM execution

### Fixed
- `SubAgentConfig.skill_dirs` parameter now properly passed from Python to Rust layer
- Skill registry now correctly filters and loads tool-type skills
- All SubAgentConfig fields now have proper getter/setter methods

### Changed
- Reorganized test structure: `tests/unit/` for mock tests, `tests/integration/` for real API tests
- Removed hardcoded API keys from test files (now use environment variables)

### Security
- All test files now use environment variables for API credentials
- Added `.gitignore` to prevent accidental credential commits

## [1.4.4] - 2026-03-15

### Added
- Initial Python SDK release with PyO3 bindings
- Basic agent and session management
- Skill system support
- Orchestrator and sub-agent execution
