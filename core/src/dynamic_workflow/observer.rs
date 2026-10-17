use super::*;

pub(super) struct WorkflowProgressState {
    pub(super) plan: ExecutionPlan,
}

impl WorkflowProgressState {
    pub(super) fn new(plan: ExecutionPlan) -> Self {
        Self { plan }
    }

    pub(super) fn upsert_step(
        &mut self,
        step_id: &str,
        step_name: &str,
        input: Option<&Value>,
        status: TaskStatus,
    ) {
        let content = workflow_step_description(step_id, step_name, input);
        self.plan.upsert_step(
            Task::new(step_id.to_string(), content)
                .with_tool(step_name)
                .with_status(status),
        );
    }

    pub(super) fn mark_status(&mut self, step_id: &str, status: TaskStatus) {
        self.plan.mark_status(step_id, status);
    }

    pub(super) fn step_position(&self, step_id: &str) -> (usize, usize) {
        let total = self.plan.steps.len().max(1);
        let number = self
            .plan
            .steps
            .iter()
            .position(|task| task.id == step_id)
            .map(|idx| idx + 1)
            .unwrap_or(total);
        (number, total)
    }

    pub(super) fn step_description(&self, step_id: &str) -> String {
        self.plan
            .steps
            .iter()
            .find(|task| task.id == step_id)
            .map(|task| task.content.clone())
            .unwrap_or_else(|| step_id.to_string())
    }

    pub(super) fn tasks(&self) -> &[Task] {
        &self.plan.steps
    }

    pub(super) fn snapshot(&self) -> ExecutionPlan {
        self.plan.clone()
    }
}

pub(super) struct AgentEventFlowObserver {
    pub(super) tx: broadcast::Sender<AgentEvent>,
    pub(super) session_id: String,
    pub(super) state: Mutex<WorkflowProgressState>,
}

impl AgentEventFlowObserver {
    pub(super) fn new(
        tx: broadcast::Sender<AgentEvent>,
        session_id: String,
        plan: ExecutionPlan,
    ) -> Self {
        Self {
            tx,
            session_id,
            state: Mutex::new(WorkflowProgressState::new(plan)),
        }
    }

    pub(super) fn emit_task_update(&self, tasks: &[Task]) {
        let _ = self.tx.send(AgentEvent::TaskUpdated {
            session_id: self.session_id.clone(),
            tasks: tasks.to_vec(),
        });
    }
}

#[async_trait]
impl FlowEventObserver for AgentEventFlowObserver {
    async fn observe(&self, envelope: FlowEventEnvelope) {
        match envelope.event {
            FlowEvent::RunStarted => {
                let _ = self.tx.send(AgentEvent::PlanningStart {
                    prompt: "dynamic_workflow".to_string(),
                });
                let state = self.state.lock().await;
                if !state.plan.steps.is_empty() {
                    let plan = state.snapshot();
                    let _ = self.tx.send(AgentEvent::PlanningEnd {
                        estimated_steps: plan.steps.len(),
                        plan,
                    });
                }
            }
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                ..
            } => {
                let mut state = self.state.lock().await;
                state.upsert_step(&step_id, &step_name, Some(&input), TaskStatus::Pending);
                self.emit_task_update(state.tasks());
                let plan = state.snapshot();
                let _ = self.tx.send(AgentEvent::PlanningEnd {
                    estimated_steps: plan.steps.len(),
                    plan,
                });
            }
            FlowEvent::StepStarted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepStart {
                    description: state.step_description(&step_id),
                    step_id,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepCompleted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Completed);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Completed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepRetrying { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(state.tasks());
            }
            FlowEvent::StepFailed { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Failed);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Failed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::RunFailed { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Failed;
                    }
                }
                self.emit_task_update(state.tasks());
            }
            FlowEvent::RunCancelled { .. } | FlowEvent::RunHostShutdown { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Cancelled;
                    }
                }
                self.emit_task_update(state.tasks());
            }
            _ => {}
        }
    }
}

pub(super) fn workflow_step_description(
    step_id: &str,
    step_name: &str,
    input: Option<&Value>,
) -> String {
    if matches!(step_name, TASK_TOOL | PARALLEL_TASK_TOOL) {
        let count = input
            .and_then(|value| value.get("tasks"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if count > 0 {
            return bounded_workflow_description(&format!(
                "Fan out {count} parallel subagent task(s)"
            ));
        }
    }

    let description = input
        .and_then(|value| value.get("description").or_else(|| value.get("title")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if step_name == step_id {
                step_id.to_string()
            } else {
                format!("{step_name}: {step_id}")
            }
        });
    bounded_workflow_description(&description)
}

pub(super) fn bounded_workflow_description(value: &str) -> String {
    const MAX_DESCRIPTION_BYTES: usize = 512;
    if value.len() <= MAX_DESCRIPTION_BYTES {
        return value.to_string();
    }
    let mut end = MAX_DESCRIPTION_BYTES.saturating_sub("…".len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}
