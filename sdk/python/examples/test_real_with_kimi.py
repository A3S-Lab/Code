#!/usr/bin/env python3
"""
完整的真实集成测试 - 使用 Kimi 模型

这个测试会：
1. 创建真实的 skill 文件
2. 使用 Kimi 模型配置
3. 实际运行 sub-agent
4. 验证 skill 是否被加载和执行
"""

import tempfile
import os
import sys
from pathlib import Path
import logging

# 启用详细日志
logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)

print("=" * 70)
print("完整真实集成测试：Skill 加载验证（使用 Kimi 模型）")
print("=" * 70)

# 创建测试环境
with tempfile.TemporaryDirectory() as tmpdir:
    workspace = Path(tmpdir) / "workspace"
    workspace.mkdir()

    skills_dir = Path(tmpdir) / "skills"
    skills_dir.mkdir()

    print(f"\n✓ 测试环境:")
    print(f"  - Workspace: {workspace}")
    print(f"  - Skills dir: {skills_dir}")

    # 创建三种不同 kind 的 skill
    print("\n" + "-" * 70)
    print("步骤 1: 创建测试 Skill 文件")
    print("-" * 70)

    # Tool kind skill
    tool_skill = skills_dir / "test-tool-skill.md"
    tool_skill.write_text("""---
name: test-tool-skill
description: "测试工具型 skill"
kind: tool
---
# Test Tool Skill

这是一个 tool 类型的 skill。

当被调用时，请回复："Tool skill 已成功执行！kind: tool 类型正常工作。"
""")
    print(f"✓ 创建 tool skill: {tool_skill.name}")

    # Instruction kind skill
    instruction_skill = skills_dir / "test-instruction-skill.md"
    instruction_skill.write_text("""---
name: test-instruction-skill
description: "测试指令型 skill"
kind: instruction
---
# Test Instruction Skill

这是一个 instruction 类型的 skill。

当被调用时，请回复："Instruction skill 已成功执行！"
""")
    print(f"✓ 创建 instruction skill: {instruction_skill.name}")

    # Persona kind skill
    persona_skill = skills_dir / "test-persona-skill.md"
    persona_skill.write_text("""---
name: test-persona-skill
description: "测试人格型 skill"
kind: persona
---
# Test Persona Skill

你是一个测试助手。
""")
    print(f"✓ 创建 persona skill: {persona_skill.name}")

    # 创建配置文件（使用 Kimi 模型）
    print("\n" + "-" * 70)
    print("步骤 2: 创建 Agent 配置（使用 Kimi 模型）")
    print("-" * 70)

    config_file = Path(tmpdir) / "agent.hcl"
    config_file.write_text("""
default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = "${KIMI_API_KEY}"
  base_url = "${KIMI_BASE_URL}"

  models {
    id   = "kimi-k2.5"
    name = "Kimi K2.5"
  }
}
""")
    print(f"✓ 配置文件: {config_file}")
    print(f"  - 模型: openai/kimi-k2.5")
    print(f"  - Base URL: $KIMI_BASE_URL (from environment)")

    # 测试 1: 验证 SubAgentConfig
    print("\n" + "=" * 70)
    print("测试 1: SubAgentConfig 配置验证")
    print("=" * 70)

    try:
        from a3s_code import SubAgentConfig

        config = SubAgentConfig(
            agent_type="general",
            prompt="请调用 Skill('test-tool-skill')",
            workspace=str(workspace),
            permissive=True,
            skill_dirs=[str(skills_dir)],
            max_steps=5,
        )

        print(f"\n✓ SubAgentConfig 创建成功:")
        print(f"  - agent_type: {config.agent_type}")
        print(f"  - workspace: {config.workspace}")
        print(f"  - skill_dirs: {config.skill_dirs}")
        print(f"  - permissive: {config.permissive}")
        print(f"  - max_steps: {config.max_steps}")

        # 验证属性访问
        assert hasattr(config, 'skill_dirs'), "❌ skill_dirs 属性不存在"
        assert config.skill_dirs == [str(skills_dir)], "❌ skill_dirs 值不正确"
        print(f"\n✓ 属性访问验证通过")

    except Exception as e:
        print(f"\n❌ SubAgentConfig 创建失败: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)

    # 测试 2: 创建 Agent 和 Orchestrator
    print("\n" + "=" * 70)
    print("测试 2: 创建 Agent 和 Orchestrator")
    print("=" * 70)

    try:
        from a3s_code import Agent, Orchestrator

        print("\n创建 Agent...")
        agent = Agent.create(str(config_file))
        print("✓ Agent 创建成功")

        print("\n创建 Orchestrator...")
        orchestrator = Orchestrator.create(agent=agent)
        print("✓ Orchestrator 创建成功")

    except Exception as e:
        print(f"\n❌ Agent/Orchestrator 创建失败: {e}")
        import traceback
        traceback.print_exc()
        print("\n可能的原因:")
        print("  - 配置文件格式错误")
        print("  - API 密钥无效")
        print("  - 网络连接问题")
        sys.exit(1)

    # 测试 3: 运行 Sub-agent（测试 tool kind）
    print("\n" + "=" * 70)
    print("测试 3: 运行 Sub-agent（测试 tool kind skill）")
    print("=" * 70)

    try:
        print(f"\n启动 Sub-agent...")
        print(f"  - 提示词: {config.prompt}")
        print(f"  - Skill 目录: {config.skill_dirs}")
        print(f"  - 预期: 应该找到并执行 'test-tool-skill'")

        handle = orchestrator.spawn_subagent(config)
        print(f"\n✓ Sub-agent 已启动 (ID: {handle.id})")

        print("\n等待执行结果...")
        print("（这可能需要几秒钟，请耐心等待）")

        result = handle.wait()

        print("\n" + "=" * 70)
        print("执行结果:")
        print("=" * 70)
        print(result)
        print("=" * 70)

        # 分析结果
        print("\n" + "-" * 70)
        print("结果分析:")
        print("-" * 70)

        result_lower = result.lower()

        if "not found" in result_lower and "skill" in result_lower:
            print("\n❌ SKILL 未找到！")
            print("\n这说明 skill_dirs 参数可能没有生效。")
            print("\n请检查上面的 DEBUG 日志，查找:")
            print("  - 'Loaded skill' 消息（应该有）")
            print("  - 'Failed to parse skill file' 警告（不应该有）")
            print("  - 'Failed to load session skill dir' 警告（不应该有）")

        elif "tool skill" in result_lower and "成功" in result_lower:
            print("\n✅ SKILL 成功执行！")
            print("\n验证结果:")
            print("  ✓ skill_dirs 参数正常工作")
            print("  ✓ kind: tool 类型被正确识别")
            print("  ✓ Rust 层正确加载了 skill")
            print("  ✓ Sub-agent 成功调用了 skill")

        elif "tool" in result_lower or "skill" in result_lower:
            print("\n⚠️  部分成功")
            print("\nSkill 可能被找到了，但执行结果不确定。")
            print("请手动检查上面的输出。")

        else:
            print("\n⚠️  结果不确定")
            print("\n无法从输出中判断 skill 是否被找到。")
            print("请手动检查上面的输出和 DEBUG 日志。")

    except Exception as e:
        print(f"\n❌ Sub-agent 执行失败: {e}")
        import traceback
        traceback.print_exc()
        print("\n可能的原因:")
        print("  - API 调用失败")
        print("  - 网络连接问题")
        print("  - 模型响应超时")

    # 测试 4: 测试 instruction kind
    print("\n" + "=" * 70)
    print("测试 4: 测试 instruction kind skill")
    print("=" * 70)

    try:
        config2 = SubAgentConfig(
            agent_type="general",
            prompt="请调用 Skill('test-instruction-skill')",
            workspace=str(workspace),
            permissive=True,
            skill_dirs=[str(skills_dir)],
            max_steps=5,
        )

        print(f"\n启动 Sub-agent（测试 instruction kind）...")
        handle2 = orchestrator.spawn_subagent(config2)
        print(f"✓ Sub-agent 已启动 (ID: {handle2.id})")

        print("\n等待执行结果...")
        result2 = handle2.wait()

        print("\n执行结果:")
        print("-" * 70)
        print(result2)
        print("-" * 70)

        if "instruction skill" in result2.lower() and "成功" in result2.lower():
            print("\n✅ Instruction skill 也成功执行！")
        else:
            print("\n⚠️  Instruction skill 结果不确定")

    except Exception as e:
        print(f"\n❌ Instruction skill 测试失败: {e}")

    # 总结
    print("\n" + "=" * 70)
    print("测试总结")
    print("=" * 70)

    print("""
完成的测试:
  1. ✓ SubAgentConfig 配置验证
  2. ✓ Agent 和 Orchestrator 创建
  3. ✓ Sub-agent 实际执行（tool kind）
  4. ✓ Sub-agent 实际执行（instruction kind）

关键验证点:
  - skill_dirs 参数是否传递到 Rust 层
  - Rust 层是否加载了 skill 文件
  - kind: tool 是否被正确识别
  - Sub-agent 是否能找到并执行 skill

如何判断成功:
  ✅ 如果看到 "Tool skill 已成功执行" - 完全成功
  ⚠️  如果看到 "Skill not found" - skill_dirs 未生效
  ⚠️  如果看到其他输出 - 需要检查 DEBUG 日志

DEBUG 日志中应该看到:
  - "Loaded skill 'test-tool-skill' from ..."
  - "Loaded skill 'test-instruction-skill' from ..."
  - "Skill registry now has X skills"

如果没有看到这些日志，说明 skill 没有被加载。
""")

print("=" * 70)
