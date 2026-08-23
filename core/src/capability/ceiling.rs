use std::collections::BTreeSet;

use serde::Serialize;

use super::{CapabilityId, CapabilityScopeError, CapabilitySet, Sha256Digest, MAX_CAPABILITIES};

pub const CAPABILITY_CEILING_SCHEMA: &str = "a3s.code.capability-ceiling.v1";

/// Workspace operations a scope may expose. A child may only turn flags off.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCapabilityCeiling {
    read: bool,
    write: bool,
    execute: bool,
    search: bool,
    git: bool,
    code_intelligence: bool,
}

impl WorkspaceCapabilityCeiling {
    pub const fn none() -> Self {
        Self {
            read: false,
            write: false,
            execute: false,
            search: false,
            git: false,
            code_intelligence: false,
        }
    }

    pub const fn all() -> Self {
        Self {
            read: true,
            write: true,
            execute: true,
            search: true,
            git: true,
            code_intelligence: true,
        }
    }

    pub const fn with_read(mut self, allowed: bool) -> Self {
        self.read = allowed;
        self
    }

    pub const fn with_write(mut self, allowed: bool) -> Self {
        self.write = allowed;
        self
    }

    pub const fn with_execute(mut self, allowed: bool) -> Self {
        self.execute = allowed;
        self
    }

    pub const fn with_search(mut self, allowed: bool) -> Self {
        self.search = allowed;
        self
    }

    pub const fn with_git(mut self, allowed: bool) -> Self {
        self.git = allowed;
        self
    }

    pub const fn with_code_intelligence(mut self, allowed: bool) -> Self {
        self.code_intelligence = allowed;
        self
    }

    pub const fn read(self) -> bool {
        self.read
    }

    pub const fn write(self) -> bool {
        self.write
    }

    pub const fn execute(self) -> bool {
        self.execute
    }

    pub const fn search(self) -> bool {
        self.search
    }

    pub const fn git(self) -> bool {
        self.git
    }

    pub const fn code_intelligence(self) -> bool {
        self.code_intelligence
    }

    fn expansion_from(self, parent: Self) -> Option<&'static str> {
        [
            (self.read, parent.read, "workspace.read"),
            (self.write, parent.write, "workspace.write"),
            (self.execute, parent.execute, "workspace.execute"),
            (self.search, parent.search, "workspace.search"),
            (self.git, parent.git, "workspace.git"),
            (
                self.code_intelligence,
                parent.code_intelligence,
                "workspace.code_intelligence",
            ),
        ]
        .into_iter()
        .find_map(|(child, parent, field)| (child && !parent).then_some(field))
    }
}

impl Default for WorkspaceCapabilityCeiling {
    fn default() -> Self {
        Self::none()
    }
}

/// Parent governance bindings that every child must retain.
///
/// These flags do not replace the concrete policy providers. They record which
/// parent enforcement boundaries must remain composed into a child scope.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GovernanceCapabilityCeiling {
    permission_guard_required: bool,
    confirmation_guard_required: bool,
    security_guard_required: bool,
    budget_guard_required: bool,
    active_skill_restrictions_required: bool,
}

impl GovernanceCapabilityCeiling {
    pub const fn none_required() -> Self {
        Self {
            permission_guard_required: false,
            confirmation_guard_required: false,
            security_guard_required: false,
            budget_guard_required: false,
            active_skill_restrictions_required: false,
        }
    }

    pub const fn require_permission_guard(mut self) -> Self {
        self.permission_guard_required = true;
        self
    }

    pub const fn require_confirmation_guard(mut self) -> Self {
        self.confirmation_guard_required = true;
        self
    }

    pub const fn require_security_guard(mut self) -> Self {
        self.security_guard_required = true;
        self
    }

    pub const fn require_budget_guard(mut self) -> Self {
        self.budget_guard_required = true;
        self
    }

    pub const fn require_active_skill_restrictions(mut self) -> Self {
        self.active_skill_restrictions_required = true;
        self
    }

    pub const fn permission_guard_required(self) -> bool {
        self.permission_guard_required
    }

    pub const fn confirmation_guard_required(self) -> bool {
        self.confirmation_guard_required
    }

    pub const fn security_guard_required(self) -> bool {
        self.security_guard_required
    }

    pub const fn budget_guard_required(self) -> bool {
        self.budget_guard_required
    }

    pub const fn active_skill_restrictions_required(self) -> bool {
        self.active_skill_restrictions_required
    }

    fn expansion_from(self, parent: Self) -> Option<&'static str> {
        [
            (
                self.permission_guard_required,
                parent.permission_guard_required,
                "governance.permission_guard",
            ),
            (
                self.confirmation_guard_required,
                parent.confirmation_guard_required,
                "governance.confirmation_guard",
            ),
            (
                self.security_guard_required,
                parent.security_guard_required,
                "governance.security_guard",
            ),
            (
                self.budget_guard_required,
                parent.budget_guard_required,
                "governance.budget_guard",
            ),
            (
                self.active_skill_restrictions_required,
                parent.active_skill_restrictions_required,
                "governance.active_skill_restrictions",
            ),
        ]
        .into_iter()
        .find_map(|(child_required, parent_required, field)| {
            (parent_required && !child_required).then_some(field)
        })
    }
}

/// Numeric execution limits. `None` is unbounded and is therefore the widest
/// value for an optional duration ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityExecutionCeiling {
    max_tool_rounds: usize,
    max_parallel_tasks: usize,
    tool_timeout_ms: Option<u64>,
    llm_api_timeout_ms: Option<u64>,
    max_execution_time_ms: Option<u64>,
}

impl CapabilityExecutionCeiling {
    pub fn new(
        max_tool_rounds: usize,
        max_parallel_tasks: usize,
        tool_timeout_ms: Option<u64>,
        llm_api_timeout_ms: Option<u64>,
        max_execution_time_ms: Option<u64>,
    ) -> Result<Self, CapabilityScopeError> {
        validate_positive_usize("max_tool_rounds", max_tool_rounds)?;
        validate_positive_usize("max_parallel_tasks", max_parallel_tasks)?;
        validate_optional_positive("tool_timeout_ms", tool_timeout_ms)?;
        validate_optional_positive("llm_api_timeout_ms", llm_api_timeout_ms)?;
        validate_optional_positive("max_execution_time_ms", max_execution_time_ms)?;
        Ok(Self {
            max_tool_rounds,
            max_parallel_tasks,
            tool_timeout_ms,
            llm_api_timeout_ms,
            max_execution_time_ms,
        })
    }

    pub const fn max_tool_rounds(self) -> usize {
        self.max_tool_rounds
    }

    pub const fn max_parallel_tasks(self) -> usize {
        self.max_parallel_tasks
    }

    pub const fn tool_timeout_ms(self) -> Option<u64> {
        self.tool_timeout_ms
    }

    pub const fn llm_api_timeout_ms(self) -> Option<u64> {
        self.llm_api_timeout_ms
    }

    pub const fn max_execution_time_ms(self) -> Option<u64> {
        self.max_execution_time_ms
    }

    fn expansion_from(self, parent: Self) -> Option<&'static str> {
        if self.max_tool_rounds > parent.max_tool_rounds {
            return Some("execution.max_tool_rounds");
        }
        if self.max_parallel_tasks > parent.max_parallel_tasks {
            return Some("execution.max_parallel_tasks");
        }
        for (child, parent, field) in [
            (
                self.tool_timeout_ms,
                parent.tool_timeout_ms,
                "execution.tool_timeout_ms",
            ),
            (
                self.llm_api_timeout_ms,
                parent.llm_api_timeout_ms,
                "execution.llm_api_timeout_ms",
            ),
            (
                self.max_execution_time_ms,
                parent.max_execution_time_ms,
                "execution.max_execution_time_ms",
            ),
        ] {
            if optional_limit_expands(child, parent) {
                return Some(field);
            }
        }
        None
    }
}

/// Complete immutable authority ceiling for one catalog generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCeiling {
    schema: &'static str,
    catalog_digest: Sha256Digest,
    allowed_capabilities: BTreeSet<CapabilityId>,
    workspace: WorkspaceCapabilityCeiling,
    governance: GovernanceCapabilityCeiling,
    execution: CapabilityExecutionCeiling,
}

impl CapabilityCeiling {
    pub fn new(
        set: &CapabilitySet,
        allowed_capabilities: impl IntoIterator<Item = CapabilityId>,
        workspace: WorkspaceCapabilityCeiling,
        governance: GovernanceCapabilityCeiling,
        execution: CapabilityExecutionCeiling,
    ) -> Result<Self, CapabilityScopeError> {
        let mut allowed = BTreeSet::new();
        for capability in allowed_capabilities {
            if allowed.len() >= MAX_CAPABILITIES {
                return Err(CapabilityScopeError::BoundExceeded {
                    field: "ceiling_capabilities",
                    max: MAX_CAPABILITIES,
                });
            }
            if !set.contains(&capability) {
                return Err(CapabilityScopeError::CapabilityOutsideCatalog {
                    capability: capability.to_string(),
                    catalog_digest: set.digest().to_string(),
                });
            }
            if !allowed.insert(capability.clone()) {
                return Err(CapabilityScopeError::DuplicateCeilingCapability {
                    capability: capability.to_string(),
                });
            }
        }
        Ok(Self {
            schema: CAPABILITY_CEILING_SCHEMA,
            catalog_digest: set.digest().clone(),
            allowed_capabilities: allowed,
            workspace,
            governance,
            execution,
        })
    }

    pub fn all(
        set: &CapabilitySet,
        workspace: WorkspaceCapabilityCeiling,
        governance: GovernanceCapabilityCeiling,
        execution: CapabilityExecutionCeiling,
    ) -> Result<Self, CapabilityScopeError> {
        Self::new(
            set,
            set.iter().map(|(id, _)| id.clone()),
            workspace,
            governance,
            execution,
        )
    }

    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub fn catalog_digest(&self) -> &Sha256Digest {
        &self.catalog_digest
    }

    pub fn allows(&self, capability: &CapabilityId) -> bool {
        self.allowed_capabilities.contains(capability)
    }

    pub fn len(&self) -> usize {
        self.allowed_capabilities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.allowed_capabilities.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &CapabilityId> {
        self.allowed_capabilities.iter()
    }

    pub const fn workspace(&self) -> WorkspaceCapabilityCeiling {
        self.workspace
    }

    pub const fn governance(&self) -> GovernanceCapabilityCeiling {
        self.governance
    }

    pub const fn execution(&self) -> CapabilityExecutionCeiling {
        self.execution
    }

    pub fn ensure_within(&self, parent: &Self) -> Result<(), CapabilityScopeError> {
        if self.catalog_digest != parent.catalog_digest {
            return Err(CapabilityScopeError::CeilingCatalogMismatch);
        }
        if !self
            .allowed_capabilities
            .is_subset(&parent.allowed_capabilities)
        {
            return Err(CapabilityScopeError::CeilingExpansion {
                dimension: "capabilities",
            });
        }
        if let Some(dimension) = self.workspace.expansion_from(parent.workspace) {
            return Err(CapabilityScopeError::CeilingExpansion { dimension });
        }
        if let Some(dimension) = self.governance.expansion_from(parent.governance) {
            return Err(CapabilityScopeError::CeilingExpansion { dimension });
        }
        if let Some(dimension) = self.execution.expansion_from(parent.execution) {
            return Err(CapabilityScopeError::CeilingExpansion { dimension });
        }
        Ok(())
    }
}

fn validate_positive_usize(field: &'static str, value: usize) -> Result<(), CapabilityScopeError> {
    if value == 0 {
        return Err(CapabilityScopeError::InvalidExecutionLimit { field });
    }
    Ok(())
}

fn validate_optional_positive(
    field: &'static str,
    value: Option<u64>,
) -> Result<(), CapabilityScopeError> {
    if value == Some(0) {
        return Err(CapabilityScopeError::InvalidExecutionLimit { field });
    }
    Ok(())
}

const fn optional_limit_expands(child: Option<u64>, parent: Option<u64>) -> bool {
    match (child, parent) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(child), Some(parent)) => child > parent,
    }
}
