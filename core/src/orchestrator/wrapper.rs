//! SubAgent 包装器
//!
//! 包装 AgentLoop 执行，提供：
//! - 事件转发到 Orchestrator
//! - 控制信号处理
//! - 状态管理

use crate::error::Result;
use crate::orchestrator::{
    ControlSignal, OrchestratorEvent, SubAgentActivity, SubAgentConfig, SubAgentState,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

/// SubAgent 包装器
///
/// TODO: 完整实现需要与 AgentLoop 集成
pub struct SubAgentWrapper {
    /// SubAgent ID
    id: String,

    /// 配置
    config: SubAgentConfig,

    /// 事件发送器
    event_tx: broadcast::Sender<OrchestratorEvent>,

    /// 控制信号接收器
    control_rx: mpsc::Receiver<ControlSignal>,

    /// 状态
    state: Arc<RwLock<SubAgentState>>,

    /// 当前活动
    activity: Arc<RwLock<SubAgentActivity>>,
}

impl SubAgentWrapper {
    /// 创建新的 wrapper
    pub fn new(
        id: String,
        config: SubAgentConfig,
        event_tx: broadcast::Sender<OrchestratorEvent>,
        control_rx: mpsc::Receiver<ControlSignal>,
        state: Arc<RwLock<SubAgentState>>,
        activity: Arc<RwLock<SubAgentActivity>>,
    ) -> Self {
        Self {
            id,
            config,
            event_tx,
            control_rx,
            state,
            activity,
        }
    }

    /// 执行 SubAgent
    ///
    /// TODO: 完整实现需要：
    /// 1. 创建 AgentLoop
    /// 2. 包装执行，转发事件
    /// 3. 处理控制信号
    pub async fn execute(mut self) -> Result<String> {
        // 更新状态为运行中
        self.update_state(SubAgentState::Running).await;

        // TODO: 实际的 AgentLoop 执行
        // 这里是占位符实现
        let result = self.execute_placeholder().await;

        // 更新状态为完成
        match &result {
            Ok(output) => {
                self.update_state(SubAgentState::Completed {
                    success: true,
                    output: output.clone(),
                })
                .await;

                // 发布完成事件
                let _ = self.event_tx.send(OrchestratorEvent::SubAgentCompleted {
                    id: self.id.clone(),
                    success: true,
                    output: output.clone(),
                    duration_ms: 0, // TODO: 实际计时
                    token_usage: None,
                });
            }
            Err(e) => {
                // 如果已经是 Cancelled 状态，不要覆盖
                let current_state = self.state.read().await.clone();
                if !matches!(current_state, SubAgentState::Cancelled) {
                    self.update_state(SubAgentState::Error {
                        message: e.to_string(),
                    })
                    .await;
                }

                // 发布完成事件
                let _ = self.event_tx.send(OrchestratorEvent::SubAgentCompleted {
                    id: self.id.clone(),
                    success: false,
                    output: e.to_string(),
                    duration_ms: 0,
                    token_usage: None,
                });
            }
        }

        result
    }

    /// 占位符执行（TODO: 替换为实际的 AgentLoop 集成）
    async fn execute_placeholder(&mut self) -> Result<String> {
        // 模拟多步执行
        for step in 1..=5 {
            // 检查控制信号（非阻塞）
            while let Ok(signal) = self.control_rx.try_recv() {
                self.handle_control_signal(signal).await?;
            }

            // 如果被取消，立即返回
            if matches!(*self.state.read().await, SubAgentState::Cancelled) {
                return Err(anyhow::anyhow!("Cancelled by orchestrator").into());
            }

            // 如果被暂停，等待恢复
            while matches!(*self.state.read().await, SubAgentState::Paused) {
                *self.activity.write().await = SubAgentActivity::WaitingForControl {
                    reason: "Paused by orchestrator".to_string(),
                };
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                // 继续检查控制信号
                while let Ok(signal) = self.control_rx.try_recv() {
                    self.handle_control_signal(signal).await?;
                }
            }

            // 模拟工具调用
            *self.activity.write().await = SubAgentActivity::CallingTool {
                tool_name: "read".to_string(),
                args: serde_json::json!({"path": "/tmp/file.txt"}),
            };

            // 发布工具调用事件
            let _ = self.event_tx.send(OrchestratorEvent::ToolExecutionStarted {
                id: self.id.clone(),
                tool_id: format!("tool-{}", step),
                tool_name: "read".to_string(),
                args: serde_json::json!({"path": "/tmp/file.txt"}),
            });

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // 模拟 LLM 请求
            *self.activity.write().await = SubAgentActivity::RequestingLlm { message_count: 3 };

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // 回到空闲状态
            *self.activity.write().await = SubAgentActivity::Idle;

            // 发布进度事件
            let _ = self.event_tx.send(OrchestratorEvent::SubAgentProgress {
                id: self.id.clone(),
                step,
                total_steps: 5,
                message: format!("Step {}/5 completed", step),
            });
        }

        Ok(format!(
            "Placeholder result for SubAgent {} ({})",
            self.id, self.config.agent_type
        ))
    }

    /// 处理控制信号
    async fn handle_control_signal(&mut self, signal: ControlSignal) -> Result<()> {
        // 发布控制信号接收事件
        let _ = self
            .event_tx
            .send(OrchestratorEvent::ControlSignalReceived {
                id: self.id.clone(),
                signal: signal.clone(),
            });

        let result = match signal {
            ControlSignal::Pause => {
                self.update_state(SubAgentState::Paused).await;
                Ok(())
            }
            ControlSignal::Resume => {
                self.update_state(SubAgentState::Running).await;
                Ok(())
            }
            ControlSignal::Cancel => {
                self.update_state(SubAgentState::Cancelled).await;
                Err(anyhow::anyhow!("Cancelled by orchestrator").into())
            }
            ControlSignal::AdjustParams { .. } => {
                // TODO: 实际参数调整逻辑
                Ok(())
            }
            ControlSignal::InjectPrompt { .. } => {
                // TODO: 实际提示词注入逻辑
                Ok(())
            }
        };

        // 发布控制信号应用事件
        let _ = self.event_tx.send(OrchestratorEvent::ControlSignalApplied {
            id: self.id.clone(),
            signal,
            success: result.is_ok(),
            error: result.as_ref().err().map(|e| format!("{}", e)),
        });

        result
    }

    /// 更新状态
    async fn update_state(&self, new_state: SubAgentState) {
        let old_state = {
            let mut state = self.state.write().await;
            let old = state.clone();
            *state = new_state.clone();
            old
        };

        // 发布状态变更事件
        let _ = self.event_tx.send(OrchestratorEvent::SubAgentStateChanged {
            id: self.id.clone(),
            old_state,
            new_state,
        });
    }
}
