"""
AHP Safety Skills

这个包包含两个核心的 safety-skills，用于 AHP Server 智能体：

1. DangerousOperationDetector - 危险操作检测器
   检测工具调用中的危险操作，包括：
   - 危险命令模式
   - 敏感路径访问
   - SSRF 风险
   - 命令注入
   - 路径遍历

2. UntrustedOutputSanitizer - 不可信输出净化器
   净化工具输出，防止：
   - 提示词注入
   - PII 泄露
   - 恶意载荷
   - 社会工程攻击

使用示例：

```python
from skills import detect_dangerous_operation, sanitize_untrusted_output

# 检测危险操作
result = detect_dangerous_operation(
    tool_name="bash",
    arguments={"command": "rm -rf /"}
)

if result["is_dangerous"]:
    print(f"危险操作: {result['risk_level']}")
    for threat in result["threats"]:
        print(f"  - {threat['description']}")

# 净化输出
result = sanitize_untrusted_output(
    tool_name="read",
    output="API_KEY=sk-123456..."
)

if not result["is_safe"]:
    print(f"不安全的输出: {result['risk_level']}")

print(f"净化后: {result['sanitized_output']}")
```
"""

from .dangerous_operation_detector import (
    DangerousOperationDetector,
    detect_dangerous_operation,
)

from .untrusted_output_sanitizer import (
    UntrustedOutputSanitizer,
    sanitize_untrusted_output,
)

__all__ = [
    "DangerousOperationDetector",
    "detect_dangerous_operation",
    "UntrustedOutputSanitizer",
    "sanitize_untrusted_output",
]

__version__ = "1.0.0"
