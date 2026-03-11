#!/usr/bin/env python3
"""
Dangerous Operation Detector Skill

这是一个 safety-skill，用于检测工具调用中的危险操作。
可以被 AHP Server 智能体调用来辅助安全分析。

功能：
- 检测危险命令模式（rm -rf, dd, mkfs 等）
- 识别敏感路径访问
- 检测权限提升尝试
- 识别网络安全风险（SSRF）
- 检测命令注入
- 评估操作风险等级

返回格式：
{
    "is_dangerous": bool,
    "risk_level": "low" | "medium" | "high" | "critical",
    "threats": [{"type": str, "description": str, "pattern": str}],
    "recommendations": [str]
}
"""

import re
import json
from typing import Dict, Any, List, Tuple
from urllib.parse import urlparse


class DangerousOperationDetector:
    """危险操作检测器"""

    # 危险命令模式
    DANGEROUS_PATTERNS = [
        (r"rm\s+-rf\s+/(?!tmp|var/tmp)", "递归删除根目录", "critical"),
        (r"rm\s+-rf\s+~", "递归删除用户主目录", "critical"),
        (r"dd\s+if=.*of=/dev/(?:sd|hd|nvme)", "直接写入磁盘设备", "critical"),
        (r"mkfs\.", "格式化文件系统", "critical"),
        (r":\(\)\{.*:\|:.*\}", "Fork 炸弹", "critical"),
        (r">?\s*/dev/sd[a-z]", "写入磁盘设备", "critical"),
        (r"chmod\s+777", "设置过于宽松的权限", "high"),
        (r"chmod\s+\+s\b", "设置 SUID 位", "high"),
        (r"curl.*\|\s*(?:bash|sh)", "管道到 shell 执行", "high"),
        (r"wget.*\|\s*(?:bash|sh)", "管道到 shell 执行", "high"),
        (r"eval\s*\(", "Eval 注入风险", "high"),
        (r"exec\s*\(", "Exec 注入风险", "high"),
        (r"sudo\s+(?!-l)", "权限提升", "medium"),
        (r"su\s+-", "切换用户", "medium"),
        (r"nc\s+-[el]", "Netcat 监听模式", "high"),
        (r"ncat\s+--(?:exec|sh-exec)", "Netcat 执行模式", "high"),
        (r"/bin/(?:bash|sh|zsh)\s+-[ci]", "交互式 shell", "medium"),
    ]

    # 敏感路径
    SENSITIVE_PATHS = [
        ("/etc/passwd", "系统密码文件", "high"),
        ("/etc/shadow", "系统影子密码文件", "critical"),
        ("/etc/sudoers", "Sudo 配置文件", "high"),
        ("/root/", "Root 用户目录", "high"),
        ("~/.ssh/id_rsa", "SSH 私钥", "critical"),
        ("~/.ssh/id_ed25519", "SSH 私钥", "critical"),
        ("/proc/", "进程信息", "medium"),
        ("/sys/", "系统信息", "medium"),
        ("~/.aws/credentials", "AWS 凭证", "critical"),
        ("~/.config/gcloud/", "GCloud 凭证", "critical"),
        (".env", "环境变量文件", "high"),
        ("credentials.json", "凭证文件", "high"),
        ("id_token", "身份令牌", "high"),
    ]

    # 私有 IP 范围（用于 SSRF 检测）
    PRIVATE_IP_PATTERNS = [
        r"^10\.",
        r"^172\.(1[6-9]|2[0-9]|3[01])\.",
        r"^192\.168\.",
        r"^127\.",
        r"^169\.254\.",
        r"^::1$",
        r"^fc00:",
        r"^fe80:",
    ]

    def detect(self, tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        """
        检测工具调用中的危险操作

        Args:
            tool_name: 工具名称
            arguments: 工具参数

        Returns:
            检测结果
        """
        threats = []
        max_risk_level = "low"

        # 将参数转换为字符串进行分析
        args_str = json.dumps(arguments, ensure_ascii=False)

        # 1. 检测危险命令模式
        pattern_threats = self._detect_dangerous_patterns(args_str)
        threats.extend(pattern_threats)

        # 2. 检测敏感路径访问
        path_threats = self._detect_sensitive_paths(args_str)
        threats.extend(path_threats)

        # 3. 检测 SSRF 风险
        if tool_name in ["web_fetch", "http_request", "curl", "wget"]:
            ssrf_threats = self._detect_ssrf(arguments)
            threats.extend(ssrf_threats)

        # 4. 检测命令注入
        injection_threats = self._detect_command_injection(args_str)
        threats.extend(injection_threats)

        # 5. 检测路径遍历
        traversal_threats = self._detect_path_traversal(args_str)
        threats.extend(traversal_threats)

        # 计算最高风险等级
        risk_levels = {"low": 0, "medium": 1, "high": 2, "critical": 3}
        if threats:
            max_risk_level = max(
                (t["risk_level"] for t in threats),
                key=lambda x: risk_levels[x]
            )

        # 生成建议
        recommendations = self._generate_recommendations(threats)

        return {
            "is_dangerous": len(threats) > 0,
            "risk_level": max_risk_level,
            "threats": threats,
            "recommendations": recommendations,
            "metadata": {
                "tool_name": tool_name,
                "threat_count": len(threats),
            }
        }

    def _detect_dangerous_patterns(self, text: str) -> List[Dict[str, Any]]:
        """检测危险命令模式"""
        threats = []
        for pattern, description, risk_level in self.DANGEROUS_PATTERNS:
            matches = re.finditer(pattern, text, re.IGNORECASE)
            for match in matches:
                threats.append({
                    "type": "dangerous_command",
                    "description": description,
                    "pattern": match.group(0),
                    "risk_level": risk_level,
                })
        return threats

    def _detect_sensitive_paths(self, text: str) -> List[Dict[str, Any]]:
        """检测敏感路径访问"""
        threats = []
        for path, description, risk_level in self.SENSITIVE_PATHS:
            if path in text:
                threats.append({
                    "type": "sensitive_path_access",
                    "description": f"访问敏感路径: {description}",
                    "pattern": path,
                    "risk_level": risk_level,
                })
        return threats

    def _detect_ssrf(self, arguments: Dict[str, Any]) -> List[Dict[str, Any]]:
        """检测 SSRF（服务器端请求伪造）风险"""
        threats = []

        # 检查 URL 参数
        url = arguments.get("url") or arguments.get("uri") or arguments.get("endpoint")
        if not url:
            return threats

        try:
            parsed = urlparse(str(url))
            hostname = parsed.hostname or parsed.netloc

            # 检查是否访问私有 IP
            for pattern in self.PRIVATE_IP_PATTERNS:
                if re.match(pattern, hostname):
                    threats.append({
                        "type": "ssrf_risk",
                        "description": f"尝试访问私有 IP 地址: {hostname}",
                        "pattern": hostname,
                        "risk_level": "high",
                    })
                    break

            # 检查是否访问 localhost
            if hostname in ["localhost", "127.0.0.1", "::1", "0.0.0.0"]:
                threats.append({
                    "type": "ssrf_risk",
                    "description": f"尝试访问本地服务: {hostname}",
                    "pattern": hostname,
                    "risk_level": "medium",
                })

            # 检查是否使用 file:// 协议
            if parsed.scheme == "file":
                threats.append({
                    "type": "ssrf_risk",
                    "description": "使用 file:// 协议访问本地文件",
                    "pattern": url,
                    "risk_level": "high",
                })

        except Exception:
            pass

        return threats

    def _detect_command_injection(self, text: str) -> List[Dict[str, Any]]:
        """检测命令注入"""
        threats = []

        # 命令注入模式
        injection_patterns = [
            (r";\s*(?:rm|cat|ls|wget|curl)", "命令分隔符注入", "high"),
            (r"\|\s*(?:bash|sh|nc)", "管道注入", "high"),
            (r"&&\s*(?:rm|cat|wget)", "逻辑与注入", "high"),
            (r"`[^`]+`", "反引号命令替换", "high"),
            (r"\$\([^)]+\)", "命令替换", "medium"),
        ]

        for pattern, description, risk_level in injection_patterns:
            if re.search(pattern, text):
                threats.append({
                    "type": "command_injection",
                    "description": description,
                    "pattern": pattern,
                    "risk_level": risk_level,
                })

        return threats

    def _detect_path_traversal(self, text: str) -> List[Dict[str, Any]]:
        """检测路径遍历攻击"""
        threats = []

        # 路径遍历模式
        if re.search(r"\.\./|\.\.\\", text):
            threats.append({
                "type": "path_traversal",
                "description": "检测到路径遍历尝试 (../)",
                "pattern": "../",
                "risk_level": "high",
            })

        return threats

    def _generate_recommendations(self, threats: List[Dict[str, Any]]) -> List[str]:
        """生成安全建议"""
        recommendations = []

        threat_types = {t["type"] for t in threats}

        if "dangerous_command" in threat_types:
            recommendations.append("避免使用危险的系统命令，使用更安全的替代方案")
            recommendations.append("如果必须执行，请使用明确的路径和参数")

        if "sensitive_path_access" in threat_types:
            recommendations.append("避免访问敏感系统文件和目录")
            recommendations.append("使用最小权限原则，只访问必要的文件")

        if "ssrf_risk" in threat_types:
            recommendations.append("验证和过滤所有外部 URL")
            recommendations.append("使用白名单限制可访问的域名和 IP")

        if "command_injection" in threat_types:
            recommendations.append("对所有用户输入进行严格的验证和转义")
            recommendations.append("避免直接拼接命令字符串，使用参数化调用")

        if "path_traversal" in threat_types:
            recommendations.append("规范化和验证所有文件路径")
            recommendations.append("限制文件访问在指定的工作目录内")

        if not recommendations:
            recommendations.append("操作看起来是安全的，但仍需谨慎")

        return recommendations


# 提供简单的函数接口
def detect_dangerous_operation(tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
    """
    检测危险操作的便捷函数

    Args:
        tool_name: 工具名称
        arguments: 工具参数

    Returns:
        检测结果
    """
    detector = DangerousOperationDetector()
    return detector.detect(tool_name, arguments)


# 命令行测试
if __name__ == "__main__":
    import sys

    # 测试用例
    test_cases = [
        {
            "name": "安全操作",
            "tool": "bash",
            "args": {"command": "ls -la"},
        },
        {
            "name": "危险删除",
            "tool": "bash",
            "args": {"command": "rm -rf /"},
        },
        {
            "name": "访问敏感文件",
            "tool": "read",
            "args": {"file_path": "/etc/shadow"},
        },
        {
            "name": "SSRF 攻击",
            "tool": "web_fetch",
            "args": {"url": "http://127.0.0.1:8080/admin"},
        },
        {
            "name": "命令注入",
            "tool": "bash",
            "args": {"command": "cat file.txt; rm -rf /tmp/*"},
        },
    ]

    print("=" * 70)
    print("Dangerous Operation Detector - 测试")
    print("=" * 70)
    print()

    for test in test_cases:
        print(f"测试: {test['name']}")
        print(f"工具: {test['tool']}")
        print(f"参数: {test['args']}")
        print()

        result = detect_dangerous_operation(test["tool"], test["args"])

        print(f"结果:")
        print(f"  危险: {result['is_dangerous']}")
        print(f"  风险等级: {result['risk_level']}")
        print(f"  威胁数量: {len(result['threats'])}")

        if result['threats']:
            print(f"  威胁:")
            for threat in result['threats']:
                print(f"    - {threat['description']} (风险: {threat['risk_level']})")

        if result['recommendations']:
            print(f"  建议:")
            for rec in result['recommendations']:
                print(f"    - {rec}")

        print()
        print("-" * 70)
        print()
