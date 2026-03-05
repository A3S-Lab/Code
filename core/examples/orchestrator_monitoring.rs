//! 演示主智能体实时监控子智能体任务列表和活动
//!
//! 运行: cargo run --example orchestrator_monitoring

use a3s_code_core::orchestrator::{AgentOrchestrator, SubAgentConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== AgentTeam 实时监控演示 ===\n");

    // 1. 创建编排器
    let orchestrator = AgentOrchestrator::new_memory();
    println!("✓ 创建 AgentOrchestrator\n");

    // 2. 启动多个子智能体
    println!("启动 3 个子智能体...");

    let config1 = SubAgentConfig::new("explore", "Search for Python files")
        .with_description("探索代码库，查找所有 Python 文件")
        .with_max_steps(10);

    let config2 = SubAgentConfig::new("analyze", "Analyze code quality")
        .with_description("分析代码质量和潜在问题")
        .with_max_steps(15);

    let config3 = SubAgentConfig::new("refactor", "Refactor legacy code")
        .with_description("重构遗留代码，提高可维护性")
        .with_max_steps(20);

    let handle1 = orchestrator.spawn_subagent(config1).await?;
    let handle2 = orchestrator.spawn_subagent(config2).await?;
    let handle3 = orchestrator.spawn_subagent(config3).await?;

    println!("✓ 启动了 3 个子智能体\n");

    // 3. 实时监控任务列表
    println!("=== 实时监控任务列表 ===\n");

    for i in 1..=5 {
        println!("--- 监控快照 #{} ---", i);

        // 获取所有子智能体信息
        let subagents = orchestrator.list_subagents().await;
        println!("活跃子智能体数量: {}", orchestrator.active_count().await);
        println!();

        for info in subagents {
            println!("SubAgent: {}", info.id);
            println!("  类型: {}", info.agent_type);
            println!("  描述: {}", info.description);
            println!("  状态: {}", info.state);
            if let Some(activity) = info.current_activity {
                println!("  当前活动: {:?}", activity);
            }
            println!();
        }

        // 获取所有活跃子智能体的当前活动
        let activities = orchestrator.get_active_activities().await;
        if !activities.is_empty() {
            println!("活跃子智能体的实时活动:");
            for (id, activity) in activities {
                println!("  {}: {:?}", id, activity);
            }
            println!();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 4. 查询特定子智能体的详细信息
    println!("=== 查询特定子智能体详情 ===\n");

    if let Some(info) = orchestrator.get_subagent_info(&handle1.id).await {
        println!("SubAgent 详情:");
        println!("  ID: {}", info.id);
        println!("  类型: {}", info.agent_type);
        println!("  描述: {}", info.description);
        println!("  状态: {}", info.state);
        println!("  创建时间: {}", info.created_at);
        println!("  更新时间: {}", info.updated_at);
        if let Some(activity) = info.current_activity {
            println!("  当前活动: {:?}", activity);
        }
        println!();
    }

    // 5. 动态控制子智能体
    println!("=== 动态控制演示 ===\n");

    println!("暂停 subagent-1...");
    orchestrator.pause_subagent(&handle1.id).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 查看状态变化
    if let Some(info) = orchestrator.get_subagent_info(&handle1.id).await {
        println!("  状态: {} (应该是 Paused)", info.state);
        if let Some(activity) = info.current_activity {
            println!("  活动: {:?}", activity);
        }
    }
    println!();

    println!("恢复 subagent-1...");
    orchestrator.resume_subagent(&handle1.id).await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    if let Some(info) = orchestrator.get_subagent_info(&handle1.id).await {
        println!("  状态: {} (应该是 Running)", info.state);
    }
    println!();

    // 6. 等待所有完成
    println!("等待所有子智能体完成...");
    orchestrator.wait_all().await?;

    println!("\n=== 最终状态 ===\n");
    let final_states = orchestrator.get_all_states().await;
    for (id, state) in final_states {
        println!("{}: {:?}", id, state);
    }

    println!("\n✓ 演示完成");

    Ok(())
}
