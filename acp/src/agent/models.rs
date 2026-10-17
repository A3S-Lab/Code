//! The model catalog, model selection, and reasoning-effort budgets.

use super::*;

impl A3sCodeAgent {
    pub(super) fn available_model_infos(&self) -> Vec<acp::ModelInfo> {
        let mut infos = Vec::new();
        for (provider, model) in self.launch.config.list_models() {
            let id = format!("{}/{}", provider.name, model.id);
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            let name = model_picker_label(&provider.name, &display_name);
            let description = Some(model_picker_description(&provider.name));
            let mut info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), name);
            if let Some(description) = description {
                info = info.description(description);
            }
            info = info.meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        if infos.is_empty() {
            let id = self.launch.model_id.clone();
            let info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), id.clone())
                .meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        infos
    }

    pub(super) fn model_state_with(
        &self,
        current: &str,
        effort_token: Option<&str>,
    ) -> acp::SessionModelState {
        let mut available = self.available_model_infos();
        let current_id = if available
            .iter()
            .any(|info| info.model_id.0.as_ref() == current)
        {
            current.to_string()
        } else {
            available
                .first()
                .map(|info| info.model_id.0.to_string())
                .unwrap_or_else(|| current.to_string())
        };
        if let Some(token) = effort_token {
            if let Some(info) = available
                .iter_mut()
                .find(|info| info.model_id.0.as_ref() == current_id)
            {
                let mut meta = info.meta.clone().unwrap_or_default();
                meta.insert(
                    "reasoningEffort".into(),
                    serde_json::Value::String(token.to_string()),
                );
                info.meta = Some(meta);
            }
        }
        acp::SessionModelState::new(acp::ModelId::new(current_id), available)
    }

    /// The model picker as a session config option: clients on ACP schemas
    /// without `SessionModelState` (Harbor's Python SDK) select through it.
    pub(super) fn model_config_options(
        models: &acp::SessionModelState,
    ) -> Vec<acp::SessionConfigOption> {
        let options: Vec<acp::SessionConfigSelectOption> = models
            .available_models
            .iter()
            .map(|info| {
                acp::SessionConfigSelectOption::new(info.model_id.0.to_string(), info.name.clone())
            })
            .collect();
        vec![acp::SessionConfigOption::select(
            MODEL_CONFIG_ID,
            "Model",
            models.current_model_id.0.to_string(),
            options,
        )
        .category(acp::SessionConfigOptionCategory::Model)]
    }

    pub(super) fn known_model(&self, model_id: &str) -> bool {
        self.launch
            .config
            .list_models()
            .iter()
            .any(|(provider, model)| format!("{}/{}", provider.name, model.id) == model_id)
            || model_id == self.launch.model_id
    }
}

/// Effort selection for one session.
///
/// `token` is the provider effort parameter when the model has one.
/// `thinking_budget` and `max_tool_rounds` are the host budget. Each level
/// has its own pair. `max` and `ultracode` keep the tool-round safety ceiling
/// as their cap; every other level is strictly below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffortLimits {
    pub(super) token: &'static str,
    pub(super) thinking_budget: usize,
    pub(super) max_tool_rounds: usize,
    pub(super) max_parallel_tasks: usize,
    pub(super) max_continuation_turns: u32,
}

/// Advertise the effort values `/effort` can apply.
///
/// A native menu wins when the provider has one (GLM stays on low/high/max).
/// Every other model still gets the host budget menu, because thinking budget
/// and tool rounds apply whether or not the provider has an effort parameter.
/// Each entry includes `value` so the pager's `ReasoningEffortOption` parser
/// accepts it.
pub(super) fn reasoning_effort_catalog_meta(
    model_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let native = a3s_code_core::llm::native_effort_menu(model_id);
    let levels: &[&str] = if native.is_empty() {
        HOST_EFFORT_LEVELS
    } else {
        native
    };
    let mut meta = serde_json::Map::new();
    meta.insert(
        "supportsReasoningEffort".into(),
        serde_json::Value::Bool(!levels.is_empty()),
    );
    let efforts = levels
        .iter()
        .map(|level| {
            let mut entry = serde_json::json!({
                "value": level,
                "id": level,
                "label": level,
                "description": effort_level_description(level),
            });
            if *level == "high" {
                entry["default"] = serde_json::Value::Bool(true);
            }
            entry
        })
        .collect::<Vec<_>>();
    meta.insert("reasoningEfforts".into(), serde_json::Value::Array(efforts));
    meta
}

pub(super) fn effort_level_description(level: &str) -> &'static str {
    match level {
        "low" => "Faster, lighter reasoning",
        "medium" => "Balanced reasoning",
        "high" => "Heavy reasoning",
        "xhigh" => "Extended reasoning",
        "max" => "Maximum reasoning",
        _ => "Reasoning effort",
    }
}

/// Depth steer from a3s `BudgetProfile.guideline`. Medium has none.
pub(super) fn effort_guideline(token: &str) -> Option<&'static str> {
    match token {
        "low" => Some(
            "[effort: low] Favor speed and minimalism. Answer directly, make the smallest \
change that works (reading enough surrounding code to change it safely), and \
keep verification proportionate: still run the narrowest build/test/type-check \
that covers what you touched — just don't add checks or scope the task didn't \
warrant. Don't gold-plate.",
        ),
        "high" => Some(
            "[effort: high] Favor depth. Reason through the approach before acting. \
Depth applies to the user's outcome, not to rediscovering the running product. After \
changes, verify the narrow path you touched (build / test / type-check) and \
check the obvious edge cases, then re-read your own diff for correctness before \
finishing.",
        ),
        "xhigh" => Some(
            "[effort: xhigh] Work rigorously. Before choosing an approach, weigh at \
least one alternative. Verify thoroughly — run the relevant tests/build, probe \
edge cases and failure modes, and confirm the change actually does what was \
asked. Do a self-review pass for correctness and simplicity before concluding.",
        ),
        "max" => Some(
            "[effort: max] Maximum rigor; prefer correctness and completeness over speed. \
Decompose the problem, compare alternatives, and implement the strongest \
solution. Verify exhaustively: tests, build, edge cases, and boundary / \
adversarial inputs. Finish with a self-critique pass that actively hunts for \
what you may have missed or gotten wrong, and fix it before concluding.",
        ),
        "ultracode" => Some(
            "[ultracode] Scripted tool fan-out is available — you decide whether a turn needs \
it. Match the effort to the task: answer trivial or conversational input (a \
greeting, a single question, a one-step edit) directly, with no plan and no \
fan-out. When a task genuinely needs many coordinated tool calls, call the \
`program` tool with one sandboxed JavaScript PTC script.",
        ),
        _ => None,
    }
}

pub(super) fn effort_limits(token: &str) -> Option<EffortLimits> {
    let ceiling = a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING;
    let (token, thinking_budget, max_tool_rounds) = match token.to_ascii_lowercase().as_str() {
        "none" => ("none", 1024, 4),
        "minimal" => ("minimal", 1536, 8),
        "low" => ("low", 2048, 16),
        "medium" => ("medium", 8192, 48),
        "high" => ("high", 16_384, 128),
        "xhigh" => ("xhigh", 32_768, 512),
        "max" => ("max", 65_536, ceiling),
        "ultracode" => ("ultracode", 65_536, ceiling),
        _ => return None,
    };
    Some(EffortLimits {
        token,
        thinking_budget,
        max_tool_rounds,
        max_parallel_tasks: HOST_PARALLEL_TASKS,
        max_continuation_turns: HOST_CONTINUATION_TURNS,
    })
}

pub(super) fn effort_limits_from_meta(meta: Option<&acp::Meta>) -> Option<EffortLimits> {
    let effort = meta
        .and_then(|m| m.get("reasoningEffort"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    effort_limits(effort)
}

/// Map pager `/effort` `_meta.reasoningEffort` onto the provider effort token.
/// The pager effort surface lands with its slice; kept linked.
#[allow(dead_code)]
pub(super) fn reasoning_effort_token(meta: Option<&acp::Meta>) -> Option<&'static str> {
    effort_limits_from_meta(meta).map(|limits| limits.token)
}
