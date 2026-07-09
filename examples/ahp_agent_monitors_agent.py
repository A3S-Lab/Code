#!/usr/bin/env python3
"""
AHP Server Agent - 智能体监控智能体架构演示

使用 A3S Code Python SDK 实现的 AHP 协议服务器，通过 Kimi 模型分析业务智能体的操作。

架构：
  业务智能体 (A3S Code + Kimi)
      ↓ AHP JSON-RPC 2.0 over stdio
  AHP Server 智能体 (A3S Code + Kimi)  ← 本程序

AHP Server 使用自己的 Kimi session 分析每个工具调用，决定是否允许或阻止。

配置：通过 KIMI_API_KEY / KIMI_BASE_URL 环境变量注入凭证。
"""

import json
import sys
import os
from pathlib import Path
from datetime import datetime
from typing import Dict, Any, Optional


def _bootstrap_a3s_code():
    """添加 a3s_code 到 sys.path（如果尚未导入）"""
    try:
        import a3s_code  # noqa: F401
        return
    except ImportError:
        pass
    # 从 examples/ 向上找到 sdk/python/.venv
    examples_dir = Path(__file__).parent
    sdk_root = examples_dir.parent / "sdk" / "python"
    for version in ("3.9", "3.10", "3.11", "3.12"):
        site = sdk_root / ".venv" / "lib" / f"python{version}" / "site-packages"
        if (site / "a3s_code").exists():
            sys.path.insert(0, str(site))
            return
    raise RuntimeError(
        "a3s_code 未找到。请先构建：\n"
        "  cd crates/code/sdk/python && maturin develop"
    )


_bootstrap_a3s_code()
from a3s_code import Agent, PermissionPolicy, SessionOptions  # noqa: E402


def find_config() -> str:
    """定位 agent_kimi.acl 配置文件（与本脚本同目录）"""
    if env := os.environ.get("A3S_CONFIG"):
        return env
    script_dir = Path(__file__).parent
    candidate = script_dir / "agent_kimi.acl"
    if candidate.exists():
        return str(candidate)
    raise FileNotFoundError(f"配置文件未找到：{candidate}")


def log(msg: str):
    """输出诊断日志到 stderr（stdout 保留给 JSON-RPC）"""
    ts = datetime.now().strftime("%H:%M:%S")
    print(f"[{ts}] [AHP-Server] {msg}", file=sys.stderr, flush=True)


PRE_TOOL_PROMPT = """\
使用 /detect-dangerous-operation skill 分析以下工具调用：

工具: {tool}
参数: {args}
智能体嵌套深度: {depth}

请调用 skill 并根据分析结果决定是否允许或阻止此操作。

只返回 JSON 对象，不要 markdown，不要 JSON 外的解释：
{{
  "action": "continue",
  "reason": "..."
}}
或
{{
  "action": "block",
  "reason": "..."
}}

决策规则：
- 如果 risk_level 是 "critical"：必须 BLOCK
- 如果 risk_level 是 "high"：默认 BLOCK
- 如果 risk_level 是 "medium"：ALLOW 但记录警告
- 如果 risk_level 是 "low" 或无威胁：ALLOW
"""


class AHPServerAgent:
    """使用 A3S Code + Kimi 的 LLM 驱动 AHP 服务器"""

    def __init__(self):
        self.session = None
        self.stats = {"requests": 0, "allowed": 0, "blocked": 0}

    def _ensure_session(self):
        """懒初始化：第一次请求时才创建 session（避免启动超时）"""
        if self.session is not None:
            return
        config = find_config()
        agent = Agent.create(config)
        import tempfile
        workspace = tempfile.mkdtemp(prefix="ahp_srv_")

        # 挂载 ahp_skills 目录，让 AHP Server 智能体使用 skill 分析工具调用
        skill_dir = str(Path(__file__).parent / "ahp_skills")
        opts = SessionOptions()
        opts.skill_dirs = [skill_dir]
        opts.permission_policy = PermissionPolicy(default_decision="allow")
        self.session = agent.session(workspace, opts)
        log(f"AHP Server Agent 已就绪 (config={config}, skills={skill_dir})")

    def _llm_decide(self, prompt: str) -> Dict[str, Any]:
        """查询 LLM 并解析 JSON 响应"""
        self._ensure_session()
        try:
            result = self.session.send(prompt)
            text = result.text.strip()
            start = text.find("{")
            end = text.rfind("}") + 1
            if start >= 0 and end > start:
                return json.loads(text[start:end])
            log(f"响应中未找到 JSON: {text[:100]}")
        except Exception as e:
            log(f"LLM 错误: {e}")
        # 解析失败时的保守回退
        return {"action": "continue", "reason": "分析失败，默认允许"}

    def _handle_pre_tool_use(self, payload: Dict[str, Any], depth: int) -> Dict[str, Any]:
        tool = payload.get("tool", "unknown")
        args = payload.get("arguments", {})
        self.stats["requests"] += 1
        log(f"pre_tool_use: tool={tool} args={json.dumps(args)[:80]} depth={depth}")

        prompt = PRE_TOOL_PROMPT.format(
            tool=tool,
            args=json.dumps(args, ensure_ascii=False),
            depth=depth,
        )
        decision = self._llm_decide(prompt)
        action = decision.get("action", "continue")
        reason = decision.get("reason", "")

        if action == "block":
            self.stats["blocked"] += 1
            log(f"  已阻止: {reason}")
        else:
            self.stats["allowed"] += 1
            log(f"  已允许: {reason}")

        return {"action": action, "reason": reason}

    def _handle_notification(self, event_type: str, payload: Dict[str, Any], depth: int):
        """通知是即发即弃的 — 不发送响应"""
        if event_type == "post_action":
            tool = payload.get("tool", "?")
            output_len = len(str((payload.get("result") or {}).get("output", "")))
            log(f"post_action (通知): tool={tool} output={output_len}B depth={depth}")
        elif event_type == "session_start":
            log(f"会话开始: {payload.get('session_id', '?')} depth={depth}")
        elif event_type == "session_end":
            log(f"会话结束。最终统计: {self.stats}")

    def _handle_handshake(self, params: Dict[str, Any]) -> Dict[str, Any]:
        """处理 AHP 握手请求"""
        log(f"收到握手请求: {params}")
        return {
            "protocol_version": "2.3",
            "harness_info": {
                "name": "ahp-agent-monitors-agent",
                "version": "1.0.0",
                "capabilities": ["pre_action", "post_action"],
            },
        }

    def _handle_request(self, event_type: str, payload: Dict[str, Any], depth: int) -> Dict[str, Any]:
        """请求需要响应"""
        if event_type == "pre_action":
            decision = self._handle_pre_tool_use(payload, depth)
            if decision.get("action") == "block":
                return {"decision": "block", "reason": decision.get("reason", "blocked by AHP")}
            return {"decision": "allow", "metadata": {"reason": decision.get("reason", "")}}
        # pre_prompt 和其他阻塞事件：默认允许
        return {"decision": "allow"}

    def dispatch(self, msg: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        method = msg.get("method")
        params = msg.get("params", {})
        req_id = msg.get("id")

        # 处理握手（使用 method 字段）
        if method == "ahp/handshake":
            result = self._handle_handshake(params)
            return {"jsonrpc": "2.0", "id": req_id, "result": result}

        # 处理事件（使用 params.event_type）
        event_type = params.get("event_type", "")
        payload = params.get("payload", {})
        depth = params.get("depth", 0)

        if req_id is None:
            self._handle_notification(event_type, payload, depth)
            return None

        result = self._handle_request(event_type, payload, depth)
        return {"jsonrpc": "2.0", "id": req_id, "result": result}

    def run(self):
        log("在 stdin 上监听 JSON-RPC 2.0 消息...")

        for raw_line in sys.stdin:
            line = raw_line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
                response = self.dispatch(msg)
                if response is not None:
                    print(json.dumps(response, ensure_ascii=False), flush=True)
            except json.JSONDecodeError as e:
                log(f"JSON 解析错误: {e} | 输入: {line[:60]}")
            except Exception as e:
                log(f"调度错误: {e}")


if __name__ == "__main__":
    AHPServerAgent().run()
