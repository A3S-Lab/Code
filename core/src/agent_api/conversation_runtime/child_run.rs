//! `task` and `skill` child runs on the fact machine.
//!
//! A child run drives the same `FactRun` a session send does, on its own
//! log, and closes through the same completion gate. It has no goal, no
//! checkpoint and no pre-analysis: the parent already planned the
//! delegated work. The child loop is only the engine the run borrows its
//! configuration, tools and gate from.

use super::*;

pub(crate) struct ChildRun {
    pub(crate) agent: crate::agent::AgentLoop,
    /// Names the child's fact thread and log directory.
    pub(crate) child_id: String,
    /// The parent session whose budget the child's model calls draw on.
    pub(crate) budget_session: Option<String>,
    pub(crate) events: Option<mpsc::Sender<AgentEvent>>,
    pub(crate) cancel: tokio_util::sync::CancellationToken,
}

impl ChildRun {
    pub(crate) async fn run(self, prompt: &str) -> anyhow::Result<AgentResult> {
        let Self {
            agent,
            child_id,
            budget_session,
            events,
            cancel,
        } = self;
        let agent = agent.into_child_run();
        let workspace = agent.tool_context_handle().workspace.clone();
        let effect_dir = crate::session_folder::child_effect_log_dir(&workspace, &child_id)?;
        let log = Arc::new(
            a3s_effect::FileLog::open(&effect_dir).map_err(|error| anyhow::anyhow!(error))?,
        );
        let client = agent.scoped_llm_client_for_parts(budget_session.as_deref(), &events, &cancel);
        let executor = agent.tool_executor_handle();
        let (permission, yolo_lanes, max_tool_rounds) = agent.fact_execution_policy();
        let confirmation = agent.confirmation_provider();
        let surface = crate::fact_control::SessionSurface {
            agent,
            session_id: child_id.clone(),
            fact_log: Arc::new(std::sync::Mutex::new(Some(log))),
            checkpoint: None,
            events,
            pending_memory_telemetry: Arc::new(std::sync::Mutex::new(Vec::new())),
            streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            forced_stop: Arc::new(std::sync::Mutex::new(None)),
            cancel,
            transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
            usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
            confirmation,
            run_store: None,
            run_id: None,
            goal_tick: None,
            ledger: Arc::new(std::sync::Mutex::new(
                crate::harness_loop::MutationLedger::default(),
            )),
            reports: Arc::new(std::sync::Mutex::new(Vec::new())),
            run_control: None,
            targeted_paths: Arc::new(std::sync::Mutex::new(Vec::new())),
            harness: None,
            host_harness_registry: None,
            host_harness_assembler: None,
        };
        let run = crate::fact_control::FactRun::open_projected_for(
            &workspace,
            &child_id,
            client,
            executor,
            (permission, &yolo_lanes, max_tool_rounds),
            surface.clone(),
            crate::fact_control::RunOwner::Child { effect_dir },
        )?;
        let mut settled = run.user_text(prompt).await?;
        settled = settle_host_input(&run, settled, &surface).await?;
        let stalled = continue_incomplete(&run, &surface, &mut settled).await?;
        drop(run);
        let usage = surface
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let messages = crate::fact_control::retained_transcript(&surface)
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut result = agent_result(&settled, messages, usage);
        result.forced_stop = surface
            .forced_stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .or_else(|| budget_denied_since_last_user_message(&settled.log));
        if stalled && result.forced_stop.is_none() {
            result.text = crate::agent::AgentLoop::no_progress_failure_text().to_string();
        }
        gate_surface_result(&surface, result, None)
    }
}

/// One extra model turn when the reply looks unfinished, matching the legacy
/// loop. A repeated incomplete reply stops with the no-progress diagnostic.
async fn continue_incomplete(
    run: &crate::fact_control::FactRun,
    surface: &crate::fact_control::SessionSurface,
    settled: &mut a3s_effect::Settlement<a3s_effect::CodingView>,
) -> anyhow::Result<bool> {
    let (enabled, max_continuations, max_rounds) = surface.agent.continuation_policy();
    if !enabled {
        return Ok(false);
    }
    let mut model_turns = 1usize;
    let mut continuations = 0u32;
    let mut previous = incomplete_fingerprint(settled.view.assistant.as_deref().unwrap_or(""));
    while crate::agent::AgentLoop::looks_incomplete(settled.view.assistant.as_deref().unwrap_or(""))
    {
        if continuations >= max_continuations || model_turns >= max_rounds {
            return Ok(false);
        }
        continuations += 1;
        model_turns += 1;
        let next = run.user_text(crate::prompts::CONTINUATION).await?;
        *settled = settle_host_input(run, next, surface).await?;
        let next = incomplete_fingerprint(settled.view.assistant.as_deref().unwrap_or(""));
        if crate::agent::AgentLoop::looks_incomplete(
            settled.view.assistant.as_deref().unwrap_or(""),
        ) && next == previous
        {
            return Ok(true);
        }
        previous = next;
    }
    Ok(false)
}

fn incomplete_fingerprint(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn budget_denied_since_last_user_message(
    log: &[a3s_effect::Fact],
) -> Option<crate::harness_loop::ForcedStop> {
    let start = log
        .iter()
        .rposition(|fact| fact.kind == "user.message")
        .unwrap_or(0);
    log[start..]
        .iter()
        .any(|fact| fact.kind == "budget.denied")
        .then_some(crate::harness_loop::ForcedStop::ToolBudget)
}
