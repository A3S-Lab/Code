//! Internal tool invocation gateway shared by agent runs, orchestrators, and
//! explicit session host calls.

use super::{ToolCapabilities, ToolContext, ToolRegistry, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Weak};

/// Policy for explicit host control-plane tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostDirectPolicy {
    /// The host has already authorized the invocation, so model-facing
    /// permission and HITL gates are bypassed. Lifecycle hooks, budgets,
    /// queue/timeout, cancellation, recursive-invocation protection, and
    /// output sanitization still apply.
    TrustedControlPlane,
    /// Re-enter the same permission and HITL gate used by model and nested
    /// invocations before executing the host-requested tool.
    GovernedControlPlane,
}

/// Identifies which runtime path requested a tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvocationOrigin {
    /// A tool call emitted directly by the model.
    Agent,
    /// A tool call emitted by an orchestrator such as `batch` or `program`.
    Nested,
    /// A private implementation step owned by a built-in runtime.
    ///
    /// The enclosing public tool has already crossed the permission boundary,
    /// so this origin skips a duplicate permission/HITL decision for the
    /// implementation tool itself. Pre-tool hooks, request evidence, budgets,
    /// cancellation, recursion guards, and every tool called by that
    /// implementation remain governed normally.
    RuntimeInternal,
    /// An explicit control-plane call made through `AgentSession::tool` or a
    /// typed direct-tool helper.
    HostDirect(HostDirectPolicy),
    /// A nested call made by a built-in control-plane orchestrator that was
    /// itself invoked directly by the host.
    ///
    /// This origin cannot be constructed through the public
    /// [`super::InvocationRuntime`]. Keeping it distinct prevents arbitrary
    /// custom tools and model sub-runs from amplifying one trusted top-level
    /// call into unrestricted nested authority.
    HostDirectNested(HostDirectPolicy),
}

impl InvocationOrigin {
    pub(crate) fn is_nested(self) -> bool {
        matches!(
            self,
            Self::Nested | Self::RuntimeInternal | Self::HostDirectNested(_)
        )
    }
}

/// Terminal outcome of a governed tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolInvocationTerminal {
    Completed,
    Rejected,
    Cancelled,
    Failed,
}

/// Lifecycle states for one governed tool invocation.
///
/// The state machine is intentionally independent of any concrete tool
/// backend. Built-ins, MCP, Flow, and Use Runtime Tasks all cross the same
/// admission and terminal boundary in the scoped invoker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolInvocationState {
    Created,
    Admitted,
    GatePending,
    Running,
    Terminal(ToolInvocationTerminal),
}

/// Internal state machine for one invocation's ownership and settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolInvocationLifecycle {
    state: ToolInvocationState,
}

impl ToolInvocationLifecycle {
    pub(crate) const fn new() -> Self {
        Self {
            state: ToolInvocationState::Created,
        }
    }

    pub(crate) const fn state(self) -> ToolInvocationState {
        self.state
    }

    pub(crate) fn transition(&mut self, next: ToolInvocationState) -> Result<(), &'static str> {
        let allowed = matches!(
            (self.state, next),
            (ToolInvocationState::Created, ToolInvocationState::Admitted)
                | (
                    ToolInvocationState::Created,
                    ToolInvocationState::Terminal(_)
                )
                | (
                    ToolInvocationState::Admitted,
                    ToolInvocationState::GatePending
                )
                | (
                    ToolInvocationState::Admitted,
                    ToolInvocationState::Terminal(_)
                )
                | (
                    ToolInvocationState::GatePending,
                    ToolInvocationState::Running
                )
                | (
                    ToolInvocationState::GatePending,
                    ToolInvocationState::Terminal(_)
                )
                | (
                    ToolInvocationState::Running,
                    ToolInvocationState::Terminal(_)
                )
        );
        if !allowed {
            return Err("invalid tool invocation lifecycle transition");
        }
        self.state = next;
        Ok(())
    }

    pub(crate) fn terminal(&mut self, outcome: ToolInvocationTerminal) {
        debug_assert!(self
            .transition(ToolInvocationState::Terminal(outcome))
            .is_ok());
    }
}

/// Owned invocation data so calls can be dispatched across async tasks.
#[derive(Debug, Clone)]
pub(crate) struct ToolInvocation {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: Value,
    pub(crate) origin: InvocationOrigin,
    pub(crate) recent_tools: Vec<String>,
}

impl ToolInvocation {
    pub(crate) fn agent(
        id: impl Into<String>,
        name: impl Into<String>,
        args: Value,
        recent_tools: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
            origin: InvocationOrigin::Agent,
            recent_tools,
        }
    }

    pub(crate) fn nested(name: impl Into<String>, args: Value) -> Self {
        let name = name.into();
        Self {
            id: format!("nested-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
            origin: InvocationOrigin::Nested,
            recent_tools: Vec::new(),
        }
    }

    pub(crate) fn runtime_internal(name: impl Into<String>, args: Value) -> Self {
        let name = name.into();
        Self {
            id: format!("runtime-internal-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
            origin: InvocationOrigin::RuntimeInternal,
            recent_tools: Vec::new(),
        }
    }

    pub(crate) fn host_direct(id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
            origin: InvocationOrigin::HostDirect(HostDirectPolicy::TrustedControlPlane),
            recent_tools: Vec::new(),
        }
    }

    pub(crate) fn host_governed(
        id: impl Into<String>,
        name: impl Into<String>,
        args: Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
            origin: InvocationOrigin::HostDirect(HostDirectPolicy::GovernedControlPlane),
            recent_tools: Vec::new(),
        }
    }

    pub(crate) fn host_direct_nested(
        name: impl Into<String>,
        args: Value,
        policy: HostDirectPolicy,
    ) -> Self {
        let name = name.into();
        Self {
            id: format!("nested-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
            origin: InvocationOrigin::HostDirectNested(policy),
            recent_tools: Vec::new(),
        }
    }

    /// Derive a replay-stable identity for this logical tool request.
    ///
    /// The transport `id` is intentionally excluded: providers and nested
    /// orchestrators may assign a fresh delivery id while retrying the same
    /// name/arguments at the same scope. The caller still owns the ledger that
    /// decides whether this identity is currently claimable or completed.
    pub(crate) fn idempotency_identity(
        &self,
        scope: Option<&str>,
    ) -> Result<
        crate::execution_identity::ExecutionIdentityV1,
        crate::execution_identity::ExecutionIdentityError,
    > {
        let origin = match self.origin {
            InvocationOrigin::Agent => "agent",
            InvocationOrigin::Nested => "nested",
            InvocationOrigin::RuntimeInternal => "runtime_internal",
            InvocationOrigin::HostDirect(HostDirectPolicy::TrustedControlPlane) => {
                "host_direct_trusted"
            }
            InvocationOrigin::HostDirect(HostDirectPolicy::GovernedControlPlane) => {
                "host_direct_governed"
            }
            InvocationOrigin::HostDirectNested(HostDirectPolicy::TrustedControlPlane) => {
                "host_direct_nested_trusted"
            }
            InvocationOrigin::HostDirectNested(HostDirectPolicy::GovernedControlPlane) => {
                "host_direct_nested_governed"
            }
        };
        crate::execution_identity::ExecutionIdentityV1::derive(
            crate::execution_identity::TOOL_INVOCATION_IDENTITY_DOMAIN_V1,
            &serde_json::json!({
                "scope": scope,
                "origin": origin,
                "name": self.name,
                "args": self.args,
                "recent_tools": self.recent_tools,
            }),
        )
    }
}

/// Execution boundary used by orchestrator tools instead of a raw registry.
#[async_trait]
pub(crate) trait ToolInvoker: Send + Sync {
    async fn invoke(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolResult;

    fn available_tools(&self) -> Vec<String>;

    fn capabilities(&self, _name: &str, _args: &Value) -> Option<ToolCapabilities> {
        None
    }
}

/// Ungoverned adapter retained only for standalone low-level registry usage.
///
/// Agent runs and `AgentSession` host-direct calls install a scoped governed
/// invoker in [`ToolContext`], which takes precedence over this adapter inside
/// orchestrator tools.
struct RegistryToolInvoker {
    registry: RegistryOwnership,
}

enum RegistryOwnership {
    Standalone(Arc<ToolRegistry>),
    RegistryBound(Weak<ToolRegistry>),
}

impl RegistryOwnership {
    fn resolve(&self) -> Option<Arc<ToolRegistry>> {
        match self {
            Self::Standalone(registry) => Some(Arc::clone(registry)),
            Self::RegistryBound(registry) => registry.upgrade(),
        }
    }
}

#[async_trait]
impl ToolInvoker for RegistryToolInvoker {
    async fn invoke(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolResult {
        let Some(registry) = self.registry.resolve() else {
            return ToolResult::error(&invocation.name, "Tool registry is closed".to_string());
        };
        let invocation_ctx = match ctx.enter_tool_invocation(&invocation.name) {
            Ok(ctx) => ctx,
            Err(message) => return ToolResult::error(&invocation.name, message),
        };

        match registry
            .execute_with_context(&invocation.name, &invocation.args, &invocation_ctx)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                ToolResult::error(&invocation.name, format!("Tool execution error: {error}"))
            }
        }
    }

    fn available_tools(&self) -> Vec<String> {
        self.registry
            .resolve()
            .map_or_else(Vec::new, |registry| registry.list())
    }

    fn capabilities(&self, name: &str, args: &Value) -> Option<ToolCapabilities> {
        self.registry
            .resolve()
            .and_then(|registry| registry.capabilities(name, args))
    }
}

pub(crate) fn registry_tool_invoker(registry: Arc<ToolRegistry>) -> Arc<dyn ToolInvoker> {
    Arc::new(RegistryToolInvoker {
        registry: RegistryOwnership::Standalone(registry),
    })
}

/// Construct an invoker installed into `registry` itself.
///
/// Its back-reference is weak so `registry -> tool -> invoker -> registry`
/// cannot retain a closed executor and its session-owned resources.
pub(crate) fn registry_bound_tool_invoker(registry: Arc<ToolRegistry>) -> Arc<dyn ToolInvoker> {
    Arc::new(RegistryToolInvoker {
        registry: RegistryOwnership::RegistryBound(Arc::downgrade(&registry)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn standalone_invoker_retains_its_registry() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("standalone-registry-test")));
        let lifetime = Arc::downgrade(&registry);
        let invoker = registry_tool_invoker(registry.clone());

        drop(registry);

        assert!(lifetime.upgrade().is_some());
        assert!(invoker.available_tools().is_empty());
    }

    #[test]
    fn tool_identity_is_stable_across_delivery_ids() {
        let first = ToolInvocation::agent(
            "delivery-a",
            "read",
            serde_json::json!({"path": "src/lib.rs"}),
            vec!["batch".to_string()],
        );
        let second = ToolInvocation::agent(
            "delivery-b",
            "read",
            serde_json::json!({"path": "src/lib.rs"}),
            vec!["batch".to_string()],
        );

        assert_eq!(
            first.idempotency_identity(Some("session-1")).unwrap(),
            second.idempotency_identity(Some("session-1")).unwrap()
        );
        assert_ne!(
            first.idempotency_identity(Some("session-1")).unwrap(),
            first.idempotency_identity(Some("session-2")).unwrap()
        );
    }

    #[test]
    fn registry_bound_invoker_does_not_retain_a_closed_registry() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("registry-cycle-test")));
        let lifetime = Arc::downgrade(&registry);
        let invoker = registry_bound_tool_invoker(registry.clone());

        drop(registry);

        assert!(lifetime.upgrade().is_none());
        assert!(invoker.available_tools().is_empty());
        assert!(invoker
            .capabilities("read", &serde_json::json!({}))
            .is_none());
    }

    #[test]
    fn lifecycle_accepts_one_terminal_transition_and_rejects_late_work() {
        let mut lifecycle = ToolInvocationLifecycle::new();
        assert_eq!(lifecycle.state(), ToolInvocationState::Created);
        lifecycle.transition(ToolInvocationState::Admitted).unwrap();
        lifecycle
            .transition(ToolInvocationState::GatePending)
            .unwrap();
        lifecycle.transition(ToolInvocationState::Running).unwrap();
        lifecycle.terminal(ToolInvocationTerminal::Completed);
        assert_eq!(
            lifecycle.state(),
            ToolInvocationState::Terminal(ToolInvocationTerminal::Completed)
        );
        assert!(lifecycle.transition(ToolInvocationState::Running).is_err());
    }

    #[test]
    fn lifecycle_allows_rejection_before_execution() {
        let mut lifecycle = ToolInvocationLifecycle::new();
        lifecycle.transition(ToolInvocationState::Admitted).unwrap();
        lifecycle
            .transition(ToolInvocationState::GatePending)
            .unwrap();
        lifecycle.terminal(ToolInvocationTerminal::Rejected);
        assert_eq!(
            lifecycle.state(),
            ToolInvocationState::Terminal(ToolInvocationTerminal::Rejected)
        );
    }
}
