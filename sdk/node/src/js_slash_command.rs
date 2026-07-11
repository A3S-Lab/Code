//! JavaScript Slash Command Implementation
//!
//! Wraps JavaScript functions as SlashCommand trait implementations.

use crate::js_callback_bridge::{decode_callback_outcome, JsCallbackOutcome};
use a3s_code_core::commands::{
    CommandContext as RustCommandContext, CommandOutput as RustCommandOutput,
    SlashCommand as RustSlashCommand,
};
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::ValueType;
use std::sync::Arc;

/// JavaScript-backed slash command.
///
/// Wraps a JavaScript function as a `SlashCommand` trait implementation.
/// The handler is called via a threadsafe function that bridges Rust → JS.
pub struct JsSlashCommand {
    pub name: String,
    pub description: String,
    pub handler: Arc<ThreadsafeFunction<(String, RustCommandContext), ErrorStrategy::Fatal>>,
    pub timeout_ms: u64,
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
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);

        let status = handler.call_with_return_value(
            (args, ctx),
            ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: napi::JsUnknown| {
                // Always return Ok from a TSFN conversion callback. Returning
                // a napi error here escalates to napi_fatal_error.
                let result = match decode_callback_outcome(ret) {
                    JsCallbackOutcome::Returned(value) => {
                        if matches!(value.get_type(), Ok(ValueType::String)) {
                            let string = unsafe { value.cast::<napi::JsString>() };
                            string
                                .into_utf8()
                                .and_then(|value| value.into_owned())
                                .map_err(|error| format!("invalid command return: {error}"))
                        } else {
                            Err("command callback must return a string".to_string())
                        }
                    }
                    JsCallbackOutcome::Failed(error) => {
                        Err(format!("command callback failed: {error}"))
                    }
                };
                let _ = tx.send(result);
                Ok(())
            },
        );

        if status != napi::Status::Ok {
            return RustCommandOutput::text(format!(
                "Command '{}' failed: handler could not be queued ({status:?})",
                self.name
            ));
        }

        match rx.recv_timeout(std::time::Duration::from_millis(self.timeout_ms)) {
            Ok(Ok(value)) => RustCommandOutput::text(value),
            Ok(Err(error)) => {
                RustCommandOutput::text(format!("Command '{}' failed: {error}", self.name))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => RustCommandOutput::text(format!(
                "Command '{}' failed: handler timed out after {}ms",
                self.name, self.timeout_ms
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                RustCommandOutput::text(format!(
                    "Command '{}' failed: handler did not return a value",
                    self.name
                ))
            }
        }
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
