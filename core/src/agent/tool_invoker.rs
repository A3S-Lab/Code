//! Scoped governance kernel for model, nested orchestrator, and host-direct
//! tool invocations.

use super::tool_result_runtime::NormalizedToolResult;
use super::{AgentEvent, AgentLoop};
use crate::budget::BudgetDecision;
use crate::safety_gate::{
    ToolGateApproval, ToolGateDecision, ToolGateDenial, ToolGateInput, ToolSafetyGate,
};
use crate::tool_confirmation::{
    ToolConfirmationRequest, ToolConfirmationResolution, ToolConfirmationRuntime,
};
use crate::tools::{
    HostDirectPolicy, InvocationOrigin, ToolCapabilities, ToolContext, ToolInvocation, ToolInvoker,
    ToolResult,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

struct ScopedToolInvoker {
    agent: AgentLoop,
    session_id: String,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
}

impl ScopedToolInvoker {
    fn new(
        agent: &AgentLoop,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Self {
        Self {
            agent: agent.clone(),
            session_id: session_id.unwrap_or("").to_string(),
            event_tx: event_tx.clone(),
        }
    }

    async fn decide_gate(&self, invocation: &ToolInvocation) -> ToolGateDecision {
        let pre_tool_block = match self
            .agent
            .fire_pre_tool_use(
                &self.session_id,
                &invocation.name,
                &invocation.args,
                invocation.recent_tools.clone(),
            )
            .await
        {
            Some(crate::hooks::HookResult::Block(reason)) => Some(reason),
            _ => None,
        };

        let host_direct_policy = match invocation.origin {
            InvocationOrigin::HostDirect(policy) | InvocationOrigin::HostDirectNested(policy) => {
                Some(policy)
            }
            InvocationOrigin::Nested | InvocationOrigin::Agent => None,
        };
        if matches!(
            host_direct_policy,
            Some(HostDirectPolicy::TrustedControlPlane)
        ) {
            return match pre_tool_block {
                Some(reason) => ToolGateDecision::Deny {
                    output: format!("Tool '{}' blocked by hook: {}", invocation.name, reason),
                    event_reason: reason,
                    reason: ToolGateDenial::HookBlock,
                },
                None => ToolGateDecision::Execute {
                    reason: ToolGateApproval::HostDirectTrusted,
                },
            };
        }

        ToolSafetyGate::new(&self.agent.config)
            .decide(ToolGateInput {
                tool_name: &invocation.name,
                args: &invocation.args,
                pre_tool_block,
                tool_requires_confirmation: self
                    .agent
                    .tool_executor
                    .registry()
                    .requires_confirmation(&invocation.name, &invocation.args),
            })
            .await
    }

    async fn resolve_gate(
        &self,
        invocation: &ToolInvocation,
        ctx: &ToolContext,
        decision: ToolGateDecision,
    ) -> NormalizedToolResult {
        match decision {
            ToolGateDecision::Deny {
                output,
                event_reason,
                reason,
            } => {
                tracing::info!(
                    tool_name = invocation.name.as_str(),
                    gate_reason = reason.as_str(),
                    origin = ?invocation.origin,
                    "Tool denied by invocation gateway"
                );
                self.emit_permission_denied(invocation, event_reason).await;
                NormalizedToolResult::denied(output)
            }
            ToolGateDecision::Execute { reason } => {
                tracing::info!(
                    tool_name = invocation.name.as_str(),
                    gate_reason = reason.as_str(),
                    origin = ?invocation.origin,
                    "Tool approved by invocation gateway"
                );
                self.execute_budgeted(invocation, ctx).await
            }
            ToolGateDecision::Confirm {
                timeout_ms,
                timeout_action,
            } => {
                let confirmation = if let Some(manager) =
                    self.agent.config.confirmation_manager.as_ref()
                {
                    let runtime =
                        ToolConfirmationRuntime::new(manager.as_ref(), self.event_tx.as_ref());
                    runtime
                        .resolve_with_cancellation(
                            ToolConfirmationRequest {
                                tool_id: &invocation.id,
                                tool_name: &invocation.name,
                                args: &invocation.args,
                                timeout_ms,
                                timeout_action,
                            },
                            &ctx.cancellation_token(),
                        )
                        .await
                } else {
                    ToolConfirmationResolution::Rejected {
                        output: format!(
                            "Tool '{}' requires confirmation but no HITL confirmation manager is configured.",
                            invocation.name
                        ),
                    }
                };

                match confirmation {
                    ToolConfirmationResolution::Approved => {
                        self.execute_budgeted(invocation, ctx).await
                    }
                    ToolConfirmationResolution::Rejected { output } => {
                        NormalizedToolResult::denied(output)
                    }
                }
            }
        }
    }

    async fn execute_budgeted(
        &self,
        invocation: &ToolInvocation,
        ctx: &ToolContext,
    ) -> NormalizedToolResult {
        if let Some(denied) = self.check_before_tool(invocation).await {
            return denied;
        }

        if let Some(tx) = &self.event_tx {
            tx.send(AgentEvent::ToolExecutionStart {
                id: invocation.id.clone(),
                name: invocation.name.clone(),
                args: invocation.args.clone(),
            })
            .await
            .ok();
        }

        let preadmitted = ctx.has_model_generation_permit();
        let llm_client = if preadmitted {
            ctx.llm_client().unwrap_or_else(|| {
                self.agent.scoped_llm_client_for_parts(
                    (!self.session_id.is_empty()).then_some(self.session_id.as_str()),
                    &self.event_tx,
                    &ctx.cancellation_token(),
                )
            })
        } else {
            self.agent.scoped_llm_client_for_parts(
                (!self.session_id.is_empty()).then_some(self.session_id.as_str()),
                &self.event_tx,
                &ctx.cancellation_token(),
            )
        };
        let model_generation_admission = if preadmitted {
            ctx.model_generation_admission()
                .unwrap_or_else(|| self.agent.model_generation_admission.clone())
        } else {
            self.agent.model_generation_admission.clone()
        };
        let governed_ctx = ctx
            .clone()
            .with_llm_client(llm_client)
            .with_model_generation_admission(model_generation_admission);
        let stream_ctx = self.agent.streaming_tool_context(
            &governed_ctx,
            &self.event_tx,
            &invocation.id,
            &invocation.name,
        );
        NormalizedToolResult::from_execution(
            self.agent
                .execute_tool_queued_or_direct(&invocation.name, &invocation.args, &stream_ctx)
                .await,
        )
    }

    async fn check_before_tool(&self, invocation: &ToolInvocation) -> Option<NormalizedToolResult> {
        let guard = self.agent.config.budget_guard.as_ref()?;
        match guard
            .check_before_tool(&self.session_id, &invocation.name)
            .await
        {
            BudgetDecision::Allow => None,
            BudgetDecision::SoftLimit {
                resource,
                consumed,
                limit,
                message,
            } => {
                self.emit_budget_event(resource, "soft", consumed, limit, message)
                    .await;
                None
            }
            BudgetDecision::Deny { resource, reason } => {
                self.emit_budget_event(resource.clone(), "hard", 0.0, 0.0, Some(reason.clone()))
                    .await;
                Some(NormalizedToolResult::denied(
                    crate::error::CodeError::BudgetExhausted { resource, reason }.to_string(),
                ))
            }
        }
    }

    async fn emit_permission_denied(&self, invocation: &ToolInvocation, reason: String) {
        if let Some(tx) = &self.event_tx {
            tx.send(AgentEvent::PermissionDenied {
                tool_id: invocation.id.clone(),
                tool_name: invocation.name.clone(),
                args: invocation.args.clone(),
                reason,
            })
            .await
            .ok();
        }
    }

    async fn emit_budget_event(
        &self,
        resource: String,
        kind: &str,
        consumed: f64,
        limit: f64,
        message: Option<String>,
    ) {
        if let Some(tx) = &self.event_tx {
            tx.send(AgentEvent::BudgetThresholdHit {
                resource,
                kind: kind.to_string(),
                consumed,
                limit,
                message,
            })
            .await
            .ok();
        }
    }

    async fn emit_nested_tool_end(&self, invocation: &ToolInvocation, result: &ToolResult) {
        if !invocation.origin.is_nested() {
            return;
        }
        if let Some(tx) = &self.event_tx {
            tx.send(AgentEvent::ToolEnd {
                id: invocation.id.clone(),
                name: invocation.name.clone(),
                args: Some(invocation.args.clone()),
                output: result.output.clone(),
                exit_code: result.exit_code,
                metadata: result.metadata.clone(),
                error_kind: result.error_kind.clone(),
            })
            .await
            .ok();
        }
    }

    async fn finish(
        &self,
        invocation: &ToolInvocation,
        started: Instant,
        normalized: NormalizedToolResult,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> ToolResult {
        self.agent
            .track_tool_result(&invocation.name, &invocation.args, normalized.exit_code);

        let mut result = normalized.into_tool_result(invocation.name.clone());
        if let Some(provider) = &self.agent.config.security_provider {
            result.output = provider.sanitize_output(&result.output);
        }

        let post_hook = self.agent.fire_post_tool_use(
            &self.session_id,
            &invocation.name,
            &invocation.args,
            &result.output,
            result.exit_code == 0,
            started.elapsed().as_millis() as u64,
        );
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {}
            _ = post_hook => {}
        }
        result
    }
}

#[async_trait]
impl ToolInvoker for ScopedToolInvoker {
    async fn invoke(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolResult {
        let started = Instant::now();
        let cancellation = ctx.cancellation_token();
        let invocation_ctx = match ctx.enter_tool_invocation(&invocation.name) {
            Ok(ctx) => ctx,
            Err(message) => {
                let result = self
                    .finish(
                        &invocation,
                        started,
                        NormalizedToolResult::denied(message),
                        &cancellation,
                    )
                    .await;
                self.emit_nested_tool_end(&invocation, &result).await;
                return result;
            }
        };
        let cancellation = invocation_ctx.cancellation_token();
        if let Err(message) = self
            .agent
            .tool_executor
            .registry()
            .validate_arguments(&invocation.name, &invocation.args)
        {
            let result = self
                .finish(
                    &invocation,
                    started,
                    NormalizedToolResult::invalid_arguments(&invocation.name, message),
                    &cancellation,
                )
                .await;
            self.emit_nested_tool_end(&invocation, &result).await;
            return result;
        }
        let decision = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                None
            }
            decision = self.decide_gate(&invocation) => Some(decision),
        };
        let normalized = match decision {
            Some(decision) => {
                // Once execution starts, its deadline wrapper owns
                // cancellation and settlement. Do not drop that future from an
                // outer select: blocking VMs and process-backed tools need a
                // chance to observe their invocation token and terminate.
                self.resolve_gate(&invocation, &invocation_ctx, decision)
                    .await
            }
            None => NormalizedToolResult::denied(format!(
                "Tool '{}' cancelled by caller",
                invocation.name
            )),
        };
        let result = self
            .finish(&invocation, started, normalized, &cancellation)
            .await;
        // Tools such as `task` emit their terminal high-level event through a
        // run-owned broadcast channel, while the caller emits `ToolEnd` on the
        // runtime mpsc channel after this method returns. Acknowledging the
        // broadcast drain here preserves that causal order across channels.
        invocation_ctx.flush_agent_events().await;
        // Model and host-direct invocations have an outer runtime owner that
        // emits their ToolEnd after this method returns. Nested orchestrator
        // calls do not, so close only those lifecycles here. Besides keeping
        // start/end IDs paired, this exposes successful nested source-tool
        // metadata to delegated-task evidence collection.
        self.emit_nested_tool_end(&invocation, &result).await;
        result
    }

    fn available_tools(&self) -> Vec<String> {
        let mut tools = self.agent.tool_executor.registry().list();
        if let Some(permission_checker) = &self.agent.config.permission_checker {
            tools.retain(|tool| permission_checker.expose_to_model(tool));
        }
        tools
    }

    fn capabilities(&self, name: &str, args: &serde_json::Value) -> Option<ToolCapabilities> {
        self.agent.tool_executor.registry().capabilities(name, args)
    }
}

impl AgentLoop {
    pub(super) fn scoped_tool_invoker(
        &self,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Arc<dyn ToolInvoker> {
        Arc::new(ScopedToolInvoker::new(self, session_id, event_tx))
    }

    pub(super) async fn invoke_model_tool(
        &self,
        invocation: ToolInvocation,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> NormalizedToolResult {
        debug_assert_eq!(invocation.origin, InvocationOrigin::Agent);
        let invoker = self.scoped_tool_invoker(session_id, event_tx);
        let run_id = self
            .checkpoint_run_id
            .clone()
            .unwrap_or_else(|| format!("standalone-{}", uuid::Uuid::new_v4()));
        let run_context =
            self.invocation_context(run_id, session_id, event_tx.clone(), cancel_token.clone());
        let ctx = run_context
            .bind_tool_context(self.tool_context.clone().without_host_direct_policy())
            .with_tool_invoker(Arc::clone(&invoker));
        NormalizedToolResult::from_tool_result(invoker.invoke(invocation, &ctx).await)
    }

    pub(crate) async fn invoke_host_tool(
        &self,
        invocation: ToolInvocation,
        session_id: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
        base_context: &ToolContext,
    ) -> ToolResult {
        debug_assert!(matches!(invocation.origin, InvocationOrigin::HostDirect(_)));
        let invoker = self.scoped_tool_invoker(Some(session_id), event_tx);
        let ctx = base_context
            .clone()
            .with_session_id(session_id)
            .with_cancellation(cancel_token.clone())
            .with_host_direct_policy(HostDirectPolicy::TrustedControlPlane)
            .with_tool_invoker(Arc::clone(&invoker));
        invoker.invoke(invocation, &ctx).await
    }
}
