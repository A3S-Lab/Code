//! `use_tool` — dispatch to a discovered MCP tool.

mod input;
pub use input::{InlineMcpInvocation, UseToolInput, parse_arguments_file};
use serde::{Deserialize, Serialize};

use crate::types::output::{MCPOutput, ToolOutput};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::util::mcp_structured_content::render_structured_content;
use crate::util::mcp_truncate::{McpTruncateContext, truncate_tool_output};

/// Wire name of the MCP dispatch tool. UIs special-case it: while its
/// arguments stream, the target tool's name is still inside them, so the
/// raw name is all a renderer has.
pub const USE_TOOL_NAME: &str = "use_tool";
pub(crate) const FILE_INPUT_SUPPORTED: &str = "file_input_supported";

/// Configuration for [`UseTool`]. Controls whether the native-tool corrective error is active. When
/// `native_tool_correction` is `true` (default), `use_tool` detects native tool names via
/// [`EnabledNativeToolNames`] and returns a targeted corrective error ("call it directly").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseToolParams {
    /// Enable the native-tool corrective error. Default: `true`.
    #[serde(default = "default_true")]
    pub native_tool_correction: bool,
    // Finalization overwrites this from host capability; never persist it as user configuration.
    #[serde(default, skip_serializing)]
    pub(crate) file_input_supported: bool,
}

impl UseToolParams {
    pub fn supports_file_input(&self) -> bool {
        self.file_input_supported
    }
}

fn default_true() -> bool {
    true
}

impl Default for UseToolParams {
    fn default() -> Self {
        Self {
            native_tool_correction: true,
            file_input_supported: false,
        }
    }
}

crate::register_resource!("grok_build", "UseTool", UseToolParams);

/// Meta tool that dispatches calls to MCP tools discovered via `search_tool`. This bypasses the outer `ToolBridge` mutex and avoids deadlock.
/// `call_raw()` skips reminders/persistence so post-processing runs exactly once (via the outer `call("use_tool")`). If `InnerDispatch` is
/// absent, dispatch fails with a clear error (should never happen in production — `FinalizedToolset::call()` always sets it).
#[derive(Debug, Default)]
pub struct UseTool;

async fn dispatch_local_mcp(
    dispatch: std::sync::Arc<crate::types::resources::InnerDispatch>,
    tool_name: &str,
    tool_input: serde_json::Value,
    ctx: a3s_tool_runtime::ToolCallContext,
) -> Result<ToolOutput, a3s_tool_runtime::ToolError> {
    let tool_id = a3s_tool_protocol::ToolId::new(tool_name).map_err(|_| {
        a3s_tool_runtime::ToolError::invalid_arguments(format!("invalid tool name: '{tool_name}'"))
    })?;
    let typed = dispatch.0.call_terminal(tool_id, tool_input, ctx).await?;
    serde_json::from_value(typed.value)
        .map_err(|e| a3s_tool_runtime::ToolError::custom("output_decoding", e.to_string()))
}

fn gateway_result_is_error(result: &serde_json::Value) -> bool {
    result
        .get("isError")
        .or_else(|| result.get("is_error"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Known content blocks become text; when every block is unknown, keep the pretty-printed
/// envelope. Otherwise append `structuredContent` with the same dedupe rule as the local client.
fn gateway_result_to_text(result: serde_json::Value) -> String {
    let content = result
        .get("content")
        .and_then(|v| v.as_array())
        .map_or(&[][..], Vec::as_slice);
    let mut parts: Vec<String> = content
        .iter()
        .filter_map(|item| {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                item.get("text").and_then(|v| v.as_str()).map(str::to_owned)
            } else if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                let mime = item
                    .get("mimeType")
                    .or_else(|| item.get("mime_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("image/png");
                item.get("data")
                    .and_then(|v| v.as_str())
                    .map(|data| format!("data:{mime};base64,{data}"))
            } else if item.get("type").and_then(|v| v.as_str()) == Some("resource") {
                serde_json::to_string(item).ok()
            } else {
                None
            }
        })
        .collect();
    // Unknown-only content: return the envelope so nothing is lost (includes structuredContent).
    if parts.is_empty() && !content.is_empty() {
        return match result {
            serde_json::Value::String(s) => s,
            other => serde_json::to_string_pretty(&other).unwrap_or_default(),
        };
    }
    parts.extend(render_structured_content(
        result
            .get("structuredContent")
            .or_else(|| result.get("structured_content")),
        parts.iter().map(String::as_str),
    ));
    if !parts.is_empty() {
        return parts.join("\n");
    }

    match result {
        serde_json::Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).unwrap_or_default(),
    }
}

fn normalize_mcp_arguments(input: serde_json::Value) -> serde_json::Value {
    match input {
        serde_json::Value::String(s) => match serde_json::from_str(&s) {
            Ok(v @ serde_json::Value::Object(_)) => v,
            _ => serde_json::Value::String(s),
        },
        serde_json::Value::Null => serde_json::json!({}),
        other => other,
    }
}

fn is_local_tool_id_rejection(err: &a3s_tool_runtime::ToolError, tool_name: &str) -> bool {
    err.kind == a3s_tool_runtime::ToolErrorKind::InvalidArguments
        && err.detail == format!("invalid tool name: '{tool_name}'")
}

async fn gateway_lookup(
    ctx: &a3s_tool_runtime::ToolCallContext,
    tool_name: &str,
) -> (
    Option<crate::types::resources::ManagedGatewayToolSource>,
    Option<crate::types::resources::ManagedGatewayToolClient>,
) {
    let Some(resources) = crate::types::tool_metadata::shared_resources(ctx).ok() else {
        return (None, None);
    };
    let guard = resources.lock().await;
    let source = guard
        .get::<crate::types::resources::ManagedGatewayToolCatalog>()
        .and_then(|catalog| catalog.get(tool_name).cloned());
    let client = guard
        .get::<crate::types::resources::ManagedGatewayToolClient>()
        .cloned()
        .filter(|_| source.is_some());
    (source, client)
}

fn gateway_response_to_output(
    tool_name: &str,
    source: crate::types::resources::ManagedGatewayToolSource,
    result: serde_json::Value,
) -> ToolOutput {
    let is_error = gateway_result_is_error(&result);
    let text = gateway_result_to_text(result);
    if is_error {
        ToolOutput::MCP(MCPOutput::errored(
            tool_name.to_owned(),
            source.connector_name,
            text,
        ))
    } else {
        ToolOutput::MCP(MCPOutput::okay_output(
            tool_name.to_owned(),
            source.connector_name,
            text,
        ))
    }
}

pub async fn dispatch_mcp_tool(
    ctx: &a3s_tool_runtime::ToolCallContext,
    tool_name: &str,
    tool_input: serde_json::Value,
    caller: &str,
) -> Result<ToolOutput, a3s_tool_runtime::ToolError> {
    let tool_input = normalize_mcp_arguments(tool_input);
    let (gateway_source, gateway_client) = gateway_lookup(ctx, tool_name).await;
    let dispatch = ctx
        .extensions
        .get::<crate::types::resources::InnerDispatch>();

    if gateway_source.is_none() && dispatch.is_none() {
        return Err(a3s_tool_runtime::ToolError::invalid_arguments(format!(
            "{caller} called outside of tool execution context. inner_dispatch not set -- this is a bug."
        )));
    }

    if let Some(source) = gateway_source {
        // A gateway-catalog name can collide with a local `server__tool` MCP tool. Local wins on a name clash: probe local dispatch first and only
        // fall through to the gateway when the local side reports the tool as not found, or rejects the catalog-derived name as an invalid local
        // ToolId. A real error from a local tool that actually dispatched propagates instead of silently retrying against the gateway.
        if tool_name.contains("__")
            && let Some(dispatch) = dispatch.clone()
        {
            match dispatch_local_mcp(dispatch, tool_name, tool_input.clone(), ctx.clone()).await {
                Ok(local_output) => return Ok(local_output),
                Err(err)
                    if err.kind != a3s_tool_runtime::ToolErrorKind::NotFound
                        && !is_local_tool_id_rejection(&err, tool_name) =>
                {
                    return Err(err);
                }
                Err(_) => {}
            }
        }

        let Some(client) = gateway_client else {
            return Err(a3s_tool_runtime::ToolError::custom(
                "managed_gateway_unavailable",
                format!(
                    "Managed MCP gateway tool '{}' is indexed but no gateway client is available.",
                    tool_name
                ),
            ));
        };
        let response = client
            .0
            .call_tool(&source.call_id, tool_input, caller)
            .await?;
        tracing::debug!(
            tool_name = %tool_name,
            reauth = response.connectors_needing_reauth.len(),
            "Managed MCP gateway tool call completed"
        );
        return Ok(gateway_response_to_output(
            tool_name,
            source,
            response.result,
        ));
    }

    dispatch_local_mcp(
        dispatch.expect("dispatch is set for local MCP path"),
        tool_name,
        tool_input,
        ctx.clone(),
    )
    .await
}

/// Check MCP routing eligibility without dispatching or probing a remote tool.
///
/// # Errors
/// Rejects native or unqualified targets not present in the managed catalog.
pub async fn validate_mcp_target(
    resources: Option<&crate::types::resources::SharedResources>,
    tool_name: &str,
) -> Result<(), a3s_tool_runtime::ToolError> {
    use crate::types::resources::{EnabledNativeToolNames, ManagedGatewayToolCatalog, Params};

    let (gateway_source, is_native, search_tool_name) = if let Some(resources) = resources {
        let guard = resources.lock().await;
        let gateway_source = guard
            .get::<ManagedGatewayToolCatalog>()
            .and_then(|catalog| catalog.get(tool_name).cloned());
        let correction_enabled = guard
            .get::<Params<UseToolParams>>()
            .is_none_or(|p| p.0.native_tool_correction);
        let native = correction_enabled
            && guard
                .get::<EnabledNativeToolNames>()
                .is_some_and(|set| set.contains(tool_name));
        let st = guard
            .get::<crate::types::template_renderer::TemplateRenderer>()
            .and_then(|r| r.tool_for_kind(ToolKind::SearchTool))
            .map(str::to_string)
            .unwrap_or_else(|| "search_tool".to_string());
        (gateway_source, native, st)
    } else {
        (None, false, "search_tool".to_string())
    };

    if !tool_name.contains("__") && gateway_source.is_none() {
        return Err(if is_native {
            tracing::info!(
                tool_name = %tool_name,
                "use_tool: native tool detected, returning corrective error"
            );
            a3s_tool_runtime::ToolError::invalid_arguments(format!(
                "`{tool}` is a native tool, not an MCP integration tool. \
                 Call `{tool}` directly as its own tool call instead of \
                 routing it through `use_tool`.",
                tool = tool_name
            ))
        } else {
            a3s_tool_runtime::ToolError::invalid_arguments(format!(
                "'{}' is not a valid MCP tool name. \
                 Tool names must be qualified as `server__tool` (e.g., `linear__save_issue`). \
                 Use `{}` to discover available tools.",
                tool_name, search_tool_name
            ))
        });
    }

    Ok(())
}

impl crate::types::tool_metadata::ToolMetadata for UseTool {
    fn kind(&self) -> ToolKind {
        ToolKind::UseTool
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Call a discovered MCP integration tool with `${{ params.use_tool.tool_name }}` and \
         `${{ params.use_tool.tool_input }}`. Arguments must match the discovered schema\
         ${%- if tools.by_kind.search_tool %} from `${{ tools.by_kind.search_tool }}`${%- endif %}."
    }

    fn versioned_definition(
        &self,
        _contract_version: Option<&str>,
        client_name: &str,
        description_override: Option<&str>,
        renderer: &crate::types::template_renderer::TemplateRenderer,
        param_map: &std::collections::HashMap<String, String>,
        _input_schema: &serde_json::Value,
        effective_params: &serde_json::Value,
    ) -> crate::types::definition::ToolDefinition {
        let supports_file_input = effective_params
            .get(FILE_INPUT_SUPPORTED)
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let template = description_override.unwrap_or_else(|| {
            if supports_file_input {
                FILE_INPUT_DESCRIPTION
            } else {
                self.description_template()
            }
        });
        let description = renderer.render(template).unwrap_or_else(|error| {
            crate::types::template_renderer::strip_markers_on_render_failure(template, &error)
        });
        let schema = serde_json::json!(UseToolInput::input_schema(supports_file_input));
        crate::types::definition::ToolDefinition::function(
            client_name,
            Some(&description),
            crate::util::remap::remap_schema_properties(&schema, param_map),
        )
    }
}

const FILE_INPUT_DESCRIPTION: &str = "Call a discovered MCP integration tool.\n\n\
         Supply exactly one form: `${{ params.use_tool.tool_name }}` plus `${{ params.use_tool.tool_input }}` inline; \
         `${{ params.use_tool.tool_name }}` plus `${{ params.use_tool.tool_input_file }}` for a UTF-8 JSON arguments-only object; \
         or `${{ params.use_tool.file }}` for a UTF-8 JSON document with canonical `tool_name` and object `tool_input`. \
         File forms require Read permission, then normal MCP approval. \
         Files must be complete regular files, at most 8 MiB. Do not mix forms or delegate to another file or native tool. \
         Remote keys and JSON-encoded strings remain unchanged. Arguments must match the discovered schema\
         ${%- if tools.by_kind.search_tool %} from `${{ tools.by_kind.search_tool }}`${%- endif %}.";

impl a3s_tool_runtime::Tool for UseTool {
    type Args = UseToolInput;
    type Output = ToolOutput;

    fn id(&self) -> a3s_tool_protocol::ToolId {
        a3s_tool_protocol::ToolId::new(USE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::a3s_tool_runtime::ListToolsContext,
    ) -> a3s_tool_types::ToolDescription {
        a3s_tool_types::ToolDescription::new(
            USE_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> a3s_tool_protocol::ToolCapabilities {
        a3s_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(a3s_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    async fn run(
        &self,
        ctx: a3s_tool_runtime::ToolCallContext,
        input: UseToolInput,
    ) -> Result<ToolOutput, a3s_tool_runtime::ToolError> {
        let UseToolInput::Inline(input) = input else {
            return Err(a3s_tool_runtime::ToolError::invalid_arguments(
                "File-backed MCP invocations require supported shell preparation; this backend cannot execute unresolved file inputs",
            ));
        };
        let resources = crate::types::tool_metadata::shared_resources(&ctx).ok();
        validate_mcp_target(resources.as_ref(), &input.tool_name).await?;

        let output =
            dispatch_mcp_tool(&ctx, &input.tool_name, input.tool_input, "use_tool").await?;

        let trunc_ctx = McpTruncateContext::from_tool_ctx(&ctx, "use_tool").await;
        Ok(truncate_tool_output(output, &trunc_ctx).await)
    }
}

#[cfg(test)]
#[path = "use_tool_tests.rs"]
mod tests;
