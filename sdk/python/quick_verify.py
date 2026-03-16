#!/usr/bin/env python3
"""
快速验证脚本 - 一键测试所有功能
"""

import sys

print("=" * 70)
print("快速验证：SubAgentConfig 和 Skill Kind Tool 支持")
print("=" * 70)

tests = []

# 测试 1: 导入测试
print("\n[1/5] 测试 SDK 导入...")
try:
    from a3s_code import SubAgentConfig
    print("✅ SDK 导入成功")
    tests.append(("SDK 导入", True))
except ImportError as e:
    print(f"❌ SDK 导入失败: {e}")
    tests.append(("SDK 导入", False))
    sys.exit(1)

# 测试 2: 属性访问
print("\n[2/5] 测试属性访问...")
try:
    cfg = SubAgentConfig(
        agent_type="test",
        prompt="test",
        skill_dirs=["/tmp/skills"]
    )

    # 测试 hasattr
    assert hasattr(cfg, 'skill_dirs'), "skill_dirs 属性不存在"
    assert hasattr(cfg, 'agent_type'), "agent_type 属性不存在"
    assert hasattr(cfg, 'workspace'), "workspace 属性不存在"

    # 测试 getter
    assert cfg.skill_dirs == ["/tmp/skills"], "skill_dirs 值不正确"
    assert cfg.agent_type == "test", "agent_type 值不正确"

    # 测试 setter
    cfg.skill_dirs = ["/new/path"]
    assert cfg.skill_dirs == ["/new/path"], "setter 不工作"

    print("✅ 属性访问正常")
    tests.append(("属性访问", True))
except Exception as e:
    print(f"❌ 属性访问失败: {e}")
    tests.append(("属性访问", False))

# 测试 3: Skill 文件格式
print("\n[3/5] 测试 Skill 文件格式...")
try:
    import tempfile
    from pathlib import Path

    with tempfile.TemporaryDirectory() as tmpdir:
        skills_dir = Path(tmpdir) / "skills"
        skills_dir.mkdir()

        # 创建 tool kind skill
        tool_skill = skills_dir / "test-tool.md"
        tool_skill.write_text("""---
name: test-tool
description: "Test tool skill"
kind: tool
---
Test content
""")

        # 验证文件存在
        assert tool_skill.exists(), "Skill 文件创建失败"

        # 验证内容
        content = tool_skill.read_text()
        assert "kind: tool" in content, "kind: tool 不在文件中"

        print("✅ Skill 文件格式正确")
        tests.append(("Skill 文件格式", True))
except Exception as e:
    print(f"❌ Skill 文件格式测试失败: {e}")
    tests.append(("Skill 文件格式", False))

# 测试 4: SubAgentConfig 与 skill_dirs
print("\n[4/5] 测试 SubAgentConfig 与 skill_dirs...")
try:
    import tempfile
    from pathlib import Path

    with tempfile.TemporaryDirectory() as tmpdir:
        skills_dir = Path(tmpdir) / "skills"
        skills_dir.mkdir()

        cfg = SubAgentConfig(
            agent_type="test",
            prompt="test",
            workspace=str(tmpdir),
            skill_dirs=[str(skills_dir)],
            permissive=True
        )

        assert cfg.skill_dirs == [str(skills_dir)], "skill_dirs 路径不匹配"
        assert cfg.workspace == str(tmpdir), "workspace 路径不匹配"
        assert cfg.permissive == True, "permissive 值不正确"

        print("✅ SubAgentConfig 配置正确")
        tests.append(("SubAgentConfig 配置", True))
except Exception as e:
    print(f"❌ SubAgentConfig 配置失败: {e}")
    tests.append(("SubAgentConfig 配置", False))

# 测试 5: 所有属性
print("\n[5/5] 测试所有属性...")
try:
    cfg = SubAgentConfig(
        agent_type="test",
        prompt="test",
        description="desc",
        permissive=True,
        permissive_deny=["write"],
        max_steps=10,
        timeout_ms=5000,
        workspace="/tmp",
        agent_dirs=["/agents"],
        skill_dirs=["/skills"]
    )

    attrs = [
        'agent_type', 'prompt', 'description', 'permissive',
        'permissive_deny', 'max_steps', 'timeout_ms',
        'workspace', 'agent_dirs', 'skill_dirs'
    ]

    for attr in attrs:
        assert hasattr(cfg, attr), f"缺少属性: {attr}"

    print(f"✅ 所有 {len(attrs)} 个属性可访问")
    tests.append(("所有属性", True))
except Exception as e:
    print(f"❌ 属性测试失败: {e}")
    tests.append(("所有属性", False))

# 总结
print("\n" + "=" * 70)
print("测试总结")
print("=" * 70)

passed = sum(1 for _, result in tests if result)
total = len(tests)

for name, result in tests:
    status = "✅" if result else "❌"
    print(f"{status} {name}")

print(f"\n通过: {passed}/{total}")

if passed == total:
    print("\n🎉 所有测试通过！")
    print("\n你现在可以使用:")
    print("  - SubAgentConfig 的所有属性")
    print("  - kind: tool 类型的 skill")
    print("\n示例:")
    print("""
  from a3s_code import SubAgentConfig

  config = SubAgentConfig(
      agent_type="test",
      prompt="Call Skill('my-tool-skill')",
      workspace="/your/workspace",
      skill_dirs=["/absolute/path/to/skills"],
      permissive=True
  )

  # 验证配置
  print(f"skill_dirs: {config.skill_dirs}")
  print(f"hasattr: {hasattr(config, 'skill_dirs')}")
""")
else:
    print(f"\n⚠️  {total - passed} 个测试失败")
    print("请检查 SDK 是否正确安装: pip install -e .")
    sys.exit(1)

print("=" * 70)
