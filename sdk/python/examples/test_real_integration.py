#!/usr/bin/env python3
"""
真实集成测试：验证 Rust 层是否真的加载了 skill

这个测试需要：
1. 有效的 LLM API 配置
2. 实际运行 sub-agent
3. 检查 skill 是否被找到
"""

import tempfile
import os
from pathlib import Path

print("=" * 70)
print("真实集成测试：Skill 加载验证")
print("=" * 70)

# 检查环境变量
api_key = os.environ.get("ANTHROPIC_API_KEY")
if not api_key:
    print("\n⚠️  警告: ANTHROPIC_API_KEY 未设置")
    print("   这个测试需要有效的 API 密钥才能运行")
    print("\n跳过实际的 sub-agent 执行测试")
    print("但我们可以验证配置是否正确...")
else:
    print(f"\n✓ 检测到 API 密钥: {api_key[:10]}...")

# 创建测试环境
with tempfile.TemporaryDirectory() as tmpdir:
    workspace = Path(tmpdir) / "workspace"
    workspace.mkdir()

    skills_dir = Path(tmpdir) / "skills"
    skills_dir.mkdir()

    # 创建一个简单的 tool skill
    test_skill = skills_dir / "test-tool.md"
    test_skill.write_text("""---
name: test-tool
description: "A simple test tool"
kind: tool
---
# Test Tool

This is a test tool skill. When called, respond with "Test tool executed successfully!"
""")

    print(f"\n✓ 测试环境创建:")
    print(f"  - Workspace: {workspace}")
    print(f"  - Skills dir: {skills_dir}")
    print(f"  - Skill file: {test_skill}")

    # 创建配置文件
    config_file = Path(tmpdir) / "agent.hcl"
    config_file.write_text(f"""
llm "anthropic" "claude-sonnet-4" {{
  api_key = env("ANTHROPIC_API_KEY")
}}

default_model = "anthropic/claude-sonnet-4"
""")

    print(f"  - Config file: {config_file}")

    # 测试配置创建
    print("\n" + "-" * 70)
    print("测试 1: SubAgentConfig 创建")
    print("-" * 70)

    try:
        from a3s_code import SubAgentConfig

        config = SubAgentConfig(
            agent_type="general",
            prompt="Call Skill('test-tool')",
            workspace=str(workspace),
            permissive=True,
            skill_dirs=[str(skills_dir)],
        )

        print(f"✓ SubAgentConfig 创建成功")
        print(f"  - agent_type: {config.agent_type}")
        print(f"  - workspace: {config.workspace}")
        print(f"  - skill_dirs: {config.skill_dirs}")
        print(f"  - permissive: {config.permissive}")

        # 验证属性
        assert hasattr(config, 'skill_dirs'), "skill_dirs 属性不存在"
        assert config.skill_dirs == [str(skills_dir)], "skill_dirs 值不正确"

        print(f"\n✓ 配置验证通过")

    except Exception as e:
        print(f"❌ 配置创建失败: {e}")
        import traceback
        traceback.print_exc()
        exit(1)

    # 如果有 API 密钥，尝试实际运行
    if api_key:
        print("\n" + "-" * 70)
        print("测试 2: 实际运行 Sub-agent（需要 API 密钥）")
        print("-" * 70)

        try:
            from a3s_code import Agent, Orchestrator
            import logging

            # 启用 debug 日志
            logging.basicConfig(
                level=logging.DEBUG,
                format='%(levelname)s - %(message)s'
            )

            print("\n创建 Agent...")
            agent = Agent.create(str(config_file))
            print("✓ Agent 创建成功")

            print("\n创建 Orchestrator...")
            orchestrator = Orchestrator.create(agent=agent)
            print("✓ Orchestrator 创建成功")

            print(f"\n启动 Sub-agent...")
            print(f"  - 提示词: {config.prompt}")
            print(f"  - Skill 目录: {config.skill_dirs}")

            handle = orchestrator.spawn_subagent(config)
            print(f"✓ Sub-agent 已启动 (ID: {handle.id})")

            print("\n等待执行结果...")
            result = handle.wait()

            print("\n" + "=" * 70)
            print("执行结果:")
            print("=" * 70)
            print(result)
            print("=" * 70)

            # 检查结果
            if "not found" in result.lower():
                print("\n❌ Skill 未找到！")
                print("   这说明 skill_dirs 参数可能仍然没有生效")
                print("\n请检查 debug 日志中的:")
                print("   - 'Loaded skill' 消息")
                print("   - 'Failed to load' 警告")
            elif "test tool executed" in result.lower() or "successfully" in result.lower():
                print("\n✅ Skill 成功执行！")
                print("   skill_dirs 参数正常工作")
            else:
                print("\n⚠️  结果不确定，请手动检查")

        except Exception as e:
            print(f"\n❌ 执行失败: {e}")
            import traceback
            traceback.print_exc()
            print("\n这可能是因为:")
            print("  - API 密钥无效")
            print("  - 网络连接问题")
            print("  - 配置文件格式错误")
    else:
        print("\n" + "-" * 70)
        print("测试 2: 跳过（需要 ANTHROPIC_API_KEY）")
        print("-" * 70)
        print("\n要运行完整测试，请设置环境变量:")
        print("  export ANTHROPIC_API_KEY='your-api-key'")
        print("  python3 test_real_integration.py")

print("\n" + "=" * 70)
print("测试完成")
print("=" * 70)

if not api_key:
    print("""
配置验证: ✅ 通过
实际执行: ⏭️  跳过（需要 API 密钥）

结论:
  - SubAgentConfig 配置正确
  - skill_dirs 参数可以正常传递
  - 要验证 Rust 层是否真的加载 skill，需要实际运行 sub-agent

下一步:
  1. 设置 ANTHROPIC_API_KEY 环境变量
  2. 重新运行此脚本
  3. 检查 debug 日志中的 skill 加载消息
""")
else:
    print("\n如果 skill 未找到，请检查:")
    print("  1. Skill 文件格式是否正确（引号闭合）")
    print("  2. 是否使用了绝对路径")
    print("  3. Debug 日志中的错误消息")

print("=" * 70)
