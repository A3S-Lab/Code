# a3s-code First-Principles Optimization Plan

## Scope

Based on the analysis, 5 concrete changes to reduce feature creep and improve modularity.
All changes are within `crates/code/core/`.

## Changes

### 1. Feature-gate Memory System (`memory` feature)

**Why**: Memory module (1582 LOC) overlaps with context compaction, skills, and context-store. Most users don't need 4-type memory classification.

**What**:
- `Cargo.toml`: Add `memory = []` feature (default OFF)
- `lib.rs`: `#[cfg(feature = "memory")]` on `pub mod memory`
- `session/mod.rs`: Gate memory initialization behind `#[cfg(feature = "memory")]`, use a no-op fallback
- `agent.rs`: Gate `MemoryStored/MemoryRecalled/MemoriesSearched/MemoryCleared` events behind `#[cfg(feature = "memory")]`
- Keep `MemoryContextProvider` gated too — it's only useful with memory

### 2. Split agent.rs (5246 LOC → 3 files)

**Why**: God module violating Single Responsibility. Events, config, and the loop are independent concerns.

**What**:
- Extract `AgentEvent` enum + serialization tests → `agent_event.rs` (~310 lines)
- Extract `AgentConfig` + `AgentResult` + `ToolCommand` → keep in `agent.rs` (small)
- `AgentLoop` impl stays in `agent.rs` (core logic)
- Update `lib.rs` re-exports

### 3. Remove SkillLoad/SkillUnload hooks

**Why**: No callers outside `hooks/events.rs` itself. Skill loading is synchronous and doesn't need interception. Added "for symmetry" with no real use case.

**What**:
- Remove `SkillLoad` / `SkillUnload` from `HookEventType` enum
- Remove `SkillLoadEvent` / `SkillUnloadEvent` structs
- Remove `skill_name()` helper from `HookEvent`
- Remove related tests
- Update `HookEvent::session_id()` to remove skill branches

### 4. Migrate remaining anyhow → CodeError in agent.rs

**Why**: `anyhow` in agent.rs is the largest remaining user. The `Internal(#[from] anyhow::Error)` variant was designed as a migration bridge — time to use it.

**What**:
- Replace `use anyhow::{Context, Result}` → `use crate::error::{CodeError, Result}`
- Replace `anyhow::bail!(error)` → `return Err(CodeError::Llm(error))`
- Replace `.context("...")` → `.map_err(|e| CodeError::Llm(format!("...: {e}")))`
- Keep `anyhow` in Cargo.toml (still used by memory trait, context trait) but reduce surface

### 5. Narrow security module pub visibility

**Why**: `ToolInterceptor` overlaps with `PermissionPolicy` (both block tool calls). The security module already correctly registers itself as hooks. But the individual sub-modules are all `pub` when they should be `pub(crate)` — external consumers should only interact via `SecurityGuard`.

**What**:
- `security/mod.rs`: Change `pub mod interceptor` → `pub(crate) mod interceptor`
- Same for `sanitizer`, `taint`, `classifier`, `injection`, `audit`
- Keep `SecurityGuard`, `SecurityConfig`, `SensitivityLevel`, `RedactionStrategy` as `pub` (the facade)
- Keep `AuditEntry`, `AuditEventType`, `AuditAction` as `pub` (needed by consumers for audit log reading)

## Order of Execution

1. Split agent.rs (no behavior change, pure refactor)
2. Remove SkillLoad/SkillUnload hooks (small, clean)
3. Feature-gate memory
4. Narrow security visibility
5. Migrate anyhow in agent.rs

## NOT Doing (Deferred)

- **External Task Offloading simplification**: Requires API design discussion, too risky for this batch
- **Full anyhow removal**: Memory/context traits still use `anyhow::Result` — changing trait signatures is a breaking API change
- **Security layer consolidation**: The current hook-based architecture is actually clean; just needs visibility narrowing
