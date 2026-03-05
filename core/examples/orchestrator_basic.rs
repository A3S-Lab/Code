//! Orchestrator 使用示例

use a3s_code_core::orchestrator::{AgentOrchestrator, SubAgentConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== AgentOrchestrator 示例 ===\n");

    // 1. 创建 orchestrator (默认使用内存事件通讯)
    let orchestrator = AgentOrchestrator::new_memory();
    println!("✓ Orchestrator 已创建（使用内存事件通讯）\n");

    // 2. 订阅所有事件
    let mut events = orchestrator.subscribe_all();
    println!("✓ 已订阅所有事件\n");

    // 3. 在后台监控事件
    tokio::spawn(async move {
        println!("--- 事件监控开始 ---");
        while let Ok(event) = events.recv().await {
            println!("[Event] {} - {:?}", event.event_name(), event.subagent_id());
        }
    });

    // 4. 启动第一个 SubAgent
    println!("启动 SubAgent 1...");
    let handle1 = orchestrator
        .spawn_subagent(
            SubAgentConfig::new("general", "Analyze Python files")
                .with_description("Count Python files")
                .with_permissive(true)
                .with_max_steps(5),
        )
        .await?;
    println!("✓ SubAgent 1 已启动: {}\n", handle1.id);

    // 5. 启动第二个 SubAgent
    println!("启动 SubAgent 2...");
    let handle2 = orchestrator
        .spawn_subagent(
            SubAgentConfig::new("explore", "Find Rust files")
                .with_description("Count Rust files")
                .with_permissive(true)
                .with_max_steps(3),
        )
        .await?;
    println!("✓ SubAgent 2 已启动: {}\n", handle2.id);

    // 6. 查询状态
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    println!("--- 当前状态 ---");
    let states = orchestrator.get_all_states().await;
    for (id, state) in &states {
        println!("  {}: {}", id, state);
    }
    println!();

    // 7. 控制 SubAgent
    println!("暂停 SubAgent 1...");
    orchestrator.pause_subagent(&handle1.id).await?;
    println!("✓ SubAgent 1 已暂停\n");

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    println!("恢复 SubAgent 1...");
    orchestrator.resume_subagent(&handle1.id).await?;
    println!("✓ SubAgent 1 已恢复\n");

    // 8. 等待完成
    println!("等待所有 SubAgent 完成...");
    let result1 = handle1.wait().await?;
    let result2 = handle2.wait().await?;

    println!("\n--- 执行结果 ---");
    println!("SubAgent 1: {}", result1);
    println!("SubAgent 2: {}", result2);

    // 9. 最终状态
    println!("\n--- 最终状态 ---");
    let final_states = orchestrator.get_all_states().await;
    for (id, state) in &final_states {
        println!("  {}: {}", id, state);
    }

    println!("\n✓ 所有 SubAgent 已完成");
    println!("活跃数量: {}", orchestrator.active_count().await);

    Ok(())
}
