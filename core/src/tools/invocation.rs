//! Internal tool invocation gateway shared by agent runs, orchestrators, and
//! explicit session host calls.

use super::{ToolCapabilities, ToolContext, ToolRegistry, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

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
        matches!(self, Self::Nested | Self::HostDirectNested(_))
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
    registry: Arc<ToolRegistry>,
}

#[async_trait]
impl ToolInvoker for RegistryToolInvoker {
    async fn invoke(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolResult {
        let invocation_ctx = match ctx.enter_tool_invocation(&invocation.name) {
            Ok(ctx) => ctx,
            Err(message) => return ToolResult::error(&invocation.name, message),
        };

        match self
            .registry
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
        self.registry.list()
    }

    fn capabilities(&self, name: &str, args: &Value) -> Option<ToolCapabilities> {
        self.registry.capabilities(name, args)
    }
}

pub(crate) fn registry_tool_invoker(registry: Arc<ToolRegistry>) -> Arc<dyn ToolInvoker> {
    Arc::new(RegistryToolInvoker { registry })
}
