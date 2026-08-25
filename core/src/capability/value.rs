use std::fmt;
use std::sync::Arc;

use crate::cognitive_context::CognitiveContextSession;
use crate::commands::SlashCommand;
use crate::context::ContextProvider;
use crate::hooks::HookBinding;
use crate::mcp::McpBinding;
use crate::skills::Skill;
use crate::subagent::AgentDefinition;
use crate::tools::Tool;

use super::{CapabilityKind, FlowBinding, KnowledgeSurfaceBinding, UiBinding};

/// Closed runtime value categories accepted by the Code projection kernel.
///
/// Implementations inside trait-backed categories remain open, but callers
/// cannot insert `Any` or invent a new product category. UI values carry only
/// renderer-neutral path-free content; embedding-host authority stays outside
/// this enum.
#[derive(Clone)]
pub enum CapabilityValue {
    Tool(Arc<dyn Tool>),
    Skill(Arc<Skill>),
    Agent(Arc<AgentDefinition>),
    Command(Arc<dyn SlashCommand>),
    Hook(Arc<HookBinding>),
    Mcp(Arc<McpBinding>),
    Flow(Arc<FlowBinding>),
    KnowledgeSurface(Arc<KnowledgeSurfaceBinding>),
    Knowledge(Arc<CognitiveContextSession>),
    Ui(Arc<UiBinding>),
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
            Self::KnowledgeSurface(_) => CapabilityKind::KnowledgeSurface,
            Self::Knowledge(_) => CapabilityKind::Knowledge,
            Self::Ui(_) => CapabilityKind::Ui,
            Self::Context(_) => CapabilityKind::Context,
        }
    }

    pub(crate) fn public_name(&self) -> Option<&str> {
        match self {
            Self::Tool(value) => Some(value.name()),
            Self::Skill(value) => Some(&value.name),
            Self::Agent(value) => Some(&value.name),
            Self::Command(value) => Some(value.name()),
            Self::Hook(value) => Some(&value.hook().id),
            Self::Flow(value) => Some(value.public_name()),
            Self::Mcp(value) => Some(value.server_name()),
            Self::KnowledgeSurface(value) => Some(value.public_name()),
            Self::Knowledge(value) => Some(value.provider_name()),
            Self::Ui(value) => Some(value.public_name()),
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
