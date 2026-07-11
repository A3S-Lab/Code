"""Stable error codes attached to exceptions raised by the native core."""

from typing import Literal

CodeErrorCode = Literal[
    "CONFIG_ERROR",
    "LLM_ERROR",
    "TOOL_ERROR",
    "SESSION_ERROR",
    "SESSION_CONFIGURATION_ERROR",
    "SESSION_INITIALIZATION_ERROR",
    "ASYNC_SESSION_BUILD_REQUIRED",
    "SESSION_CLOSED",
    "SESSION_BUSY",
    "BUDGET_EXHAUSTED",
    "SECURITY_ERROR",
    "CONTEXT_ERROR",
    "MCP_ERROR",
    "QUEUE_ERROR",
    "IO_ERROR",
    "SERIALIZATION_ERROR",
    "INTERNAL_ERROR",
]

__all__ = ["CodeErrorCode"]
