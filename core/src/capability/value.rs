use std::fmt;
use std::sync::Arc;

use crate::cognitive_context::CognitiveContextSession;
use crate::commands::SlashCommand;
use crate::context::ContextProvider;
use crate::dynamic_workflow::DynamicWorkflowRuntime;
use crate::hooks::HookHandler;
use crate::mcp::McpManager;
use crate::skills::Skill;
use crate::subagent::AgentDefinition;
use crate::tools::Tool;

use super::CapabilityKind;

/// Exact connected MCP server value projected into one immutable catalog.
///
/// The manager remains owned by the MCP concern. This binding only pins the
/// server identity used by the matching descriptor; package discovery and
/// lifecycle generations remain owned by A3S Use.
pub struct McpBinding {
    server_name: Box<str>,
    manager: Arc<McpManager>,
}

impl McpBinding {
    pub fn new(server_name: impl Into<String>, manager: Arc<McpManager>) -> Self {
        Self {
            server_name: server_name.into().into_boxed_str(),
            manager,
        }
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn manager(&self) -> &McpManager {
        &self.manager
    }
}

impl fmt::Debug for McpBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpBinding")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

/// Closed runtime value categories accepted by the Code projection kernel.
///
/// Implementations inside trait-backed categories remain open, but callers
/// cannot insert `Any` or invent a new product category. UI intentionally has
/// no variant until Core owns a typed UI runtime contract; UI descriptors fail
/// projection validation instead of entering the catalog as opaque values.
#[derive(Clone)]
pub enum CapabilityValue {
    Tool(Arc<dyn Tool>),
    Skill(Arc<Skill>),
    Agent(Arc<AgentDefinition>),
    Command(Arc<dyn SlashCommand>),
    Hook(Arc<dyn HookHandler>),
    Mcp(Arc<McpBinding>),
    Flow(Arc<DynamicWorkflowRuntime>),
    Knowledge(Arc<CognitiveContextSession>),
    Context(Arc<dyn ContextProvider>),
}

impl CapabilityValue {
    pub const fn kind(&self) -> CapabilityKind {
        match self {
            Self::Tool(_) => CapabilityKind::Tool,
            Self::Skill(_) => CapabilityKind::Skill,
            Self::Agent(_) => CapabilityKind::Agent,
            Self::Command(_) => CapabilityKind::Command,
            Self::Hook(_) => CapabilityKind::Hook,
            Self::Mcp(_) => CapabilityKind::Mcp,
            Self::Flow(_) => CapabilityKind::Flow,
            Self::Knowledge(_) => CapabilityKind::Knowledge,
            Self::Context(_) => CapabilityKind::Context,
        }
    }

    pub(crate) fn public_name(&self) -> Option<&str> {
        match self {
            Self::Tool(value) => Some(value.name()),
            Self::Skill(value) => Some(&value.name),
            Self::Agent(value) => Some(&value.name),
            Self::Command(value) => Some(value.name()),
            Self::Hook(_) | Self::Flow(_) => None,
            Self::Mcp(value) => Some(value.server_name()),
            Self::Knowledge(value) => Some(value.provider_name()),
            Self::Context(value) => Some(value.name()),
        }
    }
}

impl fmt::Debug for CapabilityValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("CapabilityValue");
        debug.field("kind", &self.kind());
        if let Some(public_name) = self.public_name() {
            debug.field("public_name", &public_name);
        }
        debug.finish_non_exhaustive()
    }
}
