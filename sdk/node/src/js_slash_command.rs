//! JavaScript Slash Command Implementation
//!
//! Wraps JavaScript functions as SlashCommand trait implementations.

use a3s_code_core::commands::{
    CommandContext as RustCommandContext, CommandOutput as RustCommandOutput,
    SlashCommand as RustSlashCommand,
};
use napi::threadsafe_function::{
    ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use std::sync::Arc;

/// JavaScript-backed slash command.
///
/// Wraps a JavaScript function as a `SlashCommand` trait implementation.
/// The handler is called via a threadsafe function that bridges Rust → JS.
pub struct JsSlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Arc<ThreadsafeFunction<(String, RustCommandContext), ErrorStrategy::Fatal>>,
}

impl RustSlashCommand for JsSlashCommand {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn execute(&self, args: &str, ctx: &RustCommandContext) -> RustCommandOutput {
        let handler = self.handler.clone();
        let args = args.to_string();
        let ctx = ctx.clone();

        // Call the JavaScript handler
        let status = handler.call((args.clone(), ctx.clone()), ThreadsafeFunctionCallMode::Blocking);

        if status != napi::Status::Ok {
            return RustCommandOutput::text(format!("Command '{}' failed: {:?}", self.name, status));
        }

        // Return a success message
        RustCommandOutput::text(format!("Command '{}' executed", self.name))
    }
}

/// Convert Rust CommandContext to JavaScript object.
pub fn js_command_context_to_object(
    env: &napi::Env,
    ctx: &RustCommandContext,
) -> napi::Result<napi::JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("sessionId", env.create_string(&ctx.session_id)?)?;
    obj.set_named_property("workspace", env.create_string(&ctx.workspace)?)?;
    obj.set_named_property("model", env.create_string(&ctx.model)?)?;
    obj.set_named_property("historyLen", env.create_uint32(ctx.history_len as u32)?)?;
    obj.set_named_property("totalTokens", env.create_int64(ctx.total_tokens as i64)?)?;
    obj.set_named_property("totalCost", env.create_double(ctx.total_cost)?)?;

    let mut tool_names_arr = env.create_array(ctx.tool_names.len() as u32)?;
    for (i, name) in ctx.tool_names.iter().enumerate() {
        tool_names_arr.set(i as u32, env.create_string(name)?)?;
    }
    obj.set_named_property("toolNames", tool_names_arr)?;

    let mut mcp_servers_arr = env.create_array(ctx.mcp_servers.len() as u32)?;
    for (i, (name, count)) in ctx.mcp_servers.iter().enumerate() {
        let mut server_obj = env.create_object()?;
        server_obj.set_named_property("name", env.create_string(name)?)?;
        server_obj.set_named_property("toolCount", env.create_uint32(*count as u32)?)?;
        mcp_servers_arr.set(i as u32, server_obj)?;
    }
    obj.set_named_property("mcpServers", mcp_servers_arr)?;

    Ok(obj)
}
