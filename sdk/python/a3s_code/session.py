"""
Session — The core abstraction for A3S Code Python SDK.

A Session binds a workspace and model at creation time (immutable).
All agentic, generation, streaming, and context management calls are methods on the session.

Supports ``async with`` for automatic cleanup.

Example:
    ```python
    from a3s_code import A3sClient, create_provider

    anthropic = create_provider(name="anthropic", api_key="sk-ant-xxx")

    async with A3sClient() as client:
        async with await client.create_session(
            model=anthropic("claude-sonnet-4-20250514"),
            workspace="/project",
            system="You are a senior engineer.",
        ) as session:
            result = await session.send("Refactor the auth module")
            print(result.text)
    ```
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import (
    Any,
    AsyncIterator,
    Dict,
    List,
    Literal,
    Optional,
    TYPE_CHECKING,
    Union,
)

from .types import (
    AgenticStrategy,
    AgenticGenerateEvent,
    Usage,
    ToolCall,
    ToolResult,
)

if TYPE_CHECKING:
    from .client import A3sClient


# ============================================================================
# Types
# ============================================================================

@dataclass
class StepResult:
    """Step information from an agentic loop iteration."""
    step_index: int
    text: str
    tool_calls: List[ToolCall] = field(default_factory=list)
    tool_results: List[ToolResult] = field(default_factory=list)
    usage: Optional[Usage] = None
    finish_reason: Optional[str] = None


@dataclass
class SendResult:
    """Result from session.send()."""
    text: str
    steps: List[StepResult] = field(default_factory=list)
    tool_calls: List[ToolCall] = field(default_factory=list)
    usage: Optional[Usage] = None
    finish_reason: str = "stop"


@dataclass
class SessionStats:
    """Session statistics."""
    total_tokens: int = 0
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_cost: float = 0.0
    message_count: int = 0
    tool_call_count: int = 0


# Agent loop event types
@dataclass
class TextEvent:
    type: Literal["text"] = "text"
    content: str = ""

@dataclass
class ToolCallEvent:
    type: Literal["tool_call"] = "tool_call"
    tool_name: str = ""
    args: Dict[str, Any] = field(default_factory=dict)
    tool_call_id: str = ""

@dataclass
class ToolResultEvent:
    type: Literal["tool_result"] = "tool_result"
    tool_call_id: str = ""
    output: str = ""
    success: bool = True

@dataclass
class StepFinishEvent:
    type: Literal["step_finish"] = "step_finish"
    step_index: int = 0
    text: str = ""

@dataclass
class ErrorEvent:
    type: Literal["error"] = "error"
    message: str = ""
    recoverable: bool = False

@dataclass
class DoneEvent:
    type: Literal["done"] = "done"
    finish_reason: str = "stop"


AgentLoopEvent = Union[
    TextEvent, ToolCallEvent, ToolResultEvent,
    StepFinishEvent, ErrorEvent, DoneEvent,
]


# ============================================================================
# Session Class
# ============================================================================

class CodeSession:
    """Session — The core object for interacting with A3S Code.

    Created via ``client.create_session(model=...)``. Workspace and model are
    immutable after creation. Supports ``async with`` for automatic cleanup.
    """

    def __init__(self, client: "A3sClient", session_id: str) -> None:
        self._client = client
        self.id = session_id
        self._closed = False

    async def __aenter__(self) -> "CodeSession":
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        await self.close()

    # --------------------------------------------------------------------------
    # AgenticLoop: send() / stream()
    # --------------------------------------------------------------------------

    async def send(
        self,
        prompt: str,
        *,
        max_steps: int = 50,
        strategy: AgenticStrategy = AgenticStrategy.AUTO,
        reflection: bool = True,
        planning: bool = False,
    ) -> SendResult:
        """Send a message to the agent. Runs the server-side AgenticLoop.

        Args:
            prompt: The task prompt
            max_steps: Maximum loop iterations (default 50)
            strategy: Agentic strategy to use
            reflection: Enable reflection after tool failures
            planning: Enable planning before execution

        Returns:
            SendResult with text, steps, tool_calls, usage, finish_reason

        Example:
            ```python
            result = await session.send("Refactor the auth module")
            print(result.text)
            ```
        """
        self._ensure_open()
        resp = await self._client.agentic_generate(
            self.id, prompt,
            strategy=strategy,
            max_steps=max_steps,
            reflection=reflection,
            planning=planning,
        )
        steps = [
            StepResult(
                step_index=s.get("step_index", i),
                text=s.get("text", ""),
                tool_calls=s.get("tool_calls", []),
                tool_results=s.get("tool_results", []),
                usage=s.get("usage"),
                finish_reason=s.get("finish_reason"),
            )
            for i, s in enumerate(resp.get("steps", []))
        ]
        return SendResult(
            text=resp.get("text", ""),
            steps=steps,
            tool_calls=resp.get("tool_calls", []),
            usage=resp.get("usage"),
            finish_reason=resp.get("finish_reason", "stop"),
        )

    async def stream(
        self,
        prompt: str,
        *,
        max_steps: int = 50,
        strategy: AgenticStrategy = AgenticStrategy.AUTO,
        reflection: bool = True,
        planning: bool = False,
    ) -> AsyncIterator[AgentLoopEvent]:
        """Stream a message to the agent with real-time events.

        Args:
            prompt: The task prompt
            max_steps: Maximum loop iterations
            strategy: Agentic strategy
            reflection: Enable reflection
            planning: Enable planning

        Yields:
            AgentLoopEvent objects (TextEvent, ToolCallEvent, etc.)

        Example:
            ```python
            async for event in session.stream("Fix all TODOs"):
                if isinstance(event, TextEvent):
                    print(event.content, end="", flush=True)
            ```
        """
        self._ensure_open()
        async for event in self._client.stream_agentic_generate(
            self.id, prompt,
            strategy=strategy,
            max_steps=max_steps,
            reflection=reflection,
            planning=planning,
        ):
            mapped = self._map_event(event)
            if mapped is not None:
                yield mapped

    # --------------------------------------------------------------------------
    # Delegation (Subagents)
    # --------------------------------------------------------------------------

    async def delegate(
        self,
        agent: str,
        task: str,
        *,
        max_steps: int = 50,
    ) -> SendResult:
        """Delegate a task to a subagent.

        Args:
            agent: Agent type ('explore', 'plan', 'general')
            task: The task prompt
            max_steps: Maximum loop iterations

        Example:
            ```python
            result = await session.delegate("explore", "Find all API endpoints")
            print(result.text)
            ```
        """
        self._ensure_open()
        resp = await self._client.delegate(self.id, agent, task)
        return SendResult(
            text=resp.get("text", ""),
            tool_calls=resp.get("tool_calls", []),
            usage=resp.get("usage"),
            finish_reason=resp.get("finish_reason", "stop"),
        )

    async def delegate_stream(
        self,
        agent: str,
        task: str,
        *,
        max_steps: int = 50,
    ) -> AsyncIterator[AgentLoopEvent]:
        """Delegate a task to a subagent with streaming.

        Yields:
            AgentLoopEvent objects
        """
        self._ensure_open()
        async for event in self._client.stream_delegate(self.id, agent, task):
            mapped = self._map_event(event)
            if mapped is not None:
                yield mapped

    # --------------------------------------------------------------------------
    # Context Management
    # --------------------------------------------------------------------------

    async def get_context_usage(self) -> Dict[str, Any]:
        """Get context usage (token counts, message count)."""
        self._ensure_open()
        return await self._client.get_context_usage(self.id)

    async def compact_context(self) -> None:
        """Compact the conversation context to save tokens."""
        self._ensure_open()
        await self._client.compact_context(self.id)

    async def clear_context(self) -> None:
        """Clear conversation history."""
        self._ensure_open()
        await self._client.clear_context(self.id)

    async def get_messages(
        self, limit: Optional[int] = None, offset: Optional[int] = None
    ) -> Dict[str, Any]:
        """Get conversation messages."""
        self._ensure_open()
        return await self._client.get_messages(self.id, limit, offset)

    # --------------------------------------------------------------------------
    # Skills
    # --------------------------------------------------------------------------

    async def load_skill(self, name: str, content: Optional[str] = None) -> None:
        """Load a skill by name."""
        self._ensure_open()
        await self._client.load_skill(self.id, name, content or "")

    async def load_skills(self, directory: str, recursive: bool = True) -> List[str]:
        """Load all skills from a directory."""
        self._ensure_open()
        resp = await self._client.load_skills_from_dir(self.id, directory, recursive)
        return resp.get("loaded_skills", [])

    async def unload_skill(self, name: str) -> None:
        """Unload a skill."""
        self._ensure_open()
        await self._client.unload_skill(self.id, name)

    async def list_skills(self) -> List[Dict[str, Any]]:
        """List available skills."""
        self._ensure_open()
        return await self._client.list_skills(self.id)

    # --------------------------------------------------------------------------
    # HITL & Permissions
    # --------------------------------------------------------------------------

    async def set_confirmation(
        self,
        *,
        require_confirmation: Optional[List[str]] = None,
        auto_approve: Optional[List[str]] = None,
        timeout: int = 30000,
        timeout_action: str = "reject",
    ) -> None:
        """Set HITL confirmation policy."""
        self._ensure_open()
        from .types import ConfirmationPolicy
        policy = ConfirmationPolicy(
            enabled=True,
            require_confirm_tools=require_confirmation or [],
            auto_approve_tools=auto_approve or [],
            default_timeout_ms=timeout,
            timeout_action=timeout_action,
        )
        await self._client.set_confirmation_policy(self.id, policy)

    async def set_permissions(
        self,
        *,
        default_action: str = "allow",
        allow: Optional[List[str]] = None,
        deny: Optional[List[str]] = None,
        ask: Optional[List[str]] = None,
    ) -> None:
        """Set tool permission policy."""
        self._ensure_open()
        from .types import PermissionPolicy, PermissionRule
        policy = PermissionPolicy(
            enabled=True,
            allow=[PermissionRule(rule=r) for r in (allow or [])],
            deny=[PermissionRule(rule=r) for r in (deny or [])],
            ask=[PermissionRule(rule=r) for r in (ask or [])],
            default_decision=default_action,
        )
        await self._client.set_permission_policy(self.id, policy)

    async def confirm(self, confirmation_id: str, approved: bool, reason: str = "") -> None:
        """Respond to a confirmation request."""
        self._ensure_open()
        await self._client.confirm_tool_execution(self.id, confirmation_id, approved, reason)

    # --------------------------------------------------------------------------
    # Observability
    # --------------------------------------------------------------------------

    async def get_stats(self) -> SessionStats:
        """Get session statistics (tokens, cost, tool calls)."""
        self._ensure_open()
        cost = await self._client.get_cost_summary(self.id)
        return SessionStats(
            total_tokens=cost.get("total_tokens", 0),
            prompt_tokens=cost.get("total_prompt_tokens", 0),
            completion_tokens=cost.get("total_completion_tokens", 0),
            total_cost=cost.get("total_cost_usd", 0.0),
            message_count=cost.get("call_count", 0),
            tool_call_count=cost.get("call_count", 0),
        )

    async def get_tool_metrics(self) -> Dict[str, Any]:
        """Get per-tool execution metrics."""
        self._ensure_open()
        return await self._client.get_tool_metrics(self.id)

    # --------------------------------------------------------------------------
    # Lifecycle
    # --------------------------------------------------------------------------

    async def close(self) -> None:
        """Close the session and release server resources."""
        if self._closed:
            return
        self._closed = True
        try:
            await self._client.destroy_session(self.id)
        except Exception:
            pass

    @property
    def closed(self) -> bool:
        """Whether this session has been closed."""
        return self._closed

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError(f"Session {self.id} has been closed")

    @staticmethod
    def _map_event(event: AgenticGenerateEvent) -> Optional[AgentLoopEvent]:
        """Map a proto AgenticGenerateEvent to an SDK AgentLoopEvent."""
        etype = getattr(event, "type", None) or event.get("type", "")
        if etype == "text":
            content = getattr(event, "content", None) or event.get("text_delta", "") or event.get("content", "")
            if content:
                return TextEvent(content=content)
        elif etype == "tool_call":
            tc = getattr(event, "tool_call", None) or event.get("tool_call", {})
            return ToolCallEvent(
                tool_name=tc.get("name", "") if isinstance(tc, dict) else getattr(tc, "name", ""),
                args={},
                tool_call_id=tc.get("id", "") if isinstance(tc, dict) else getattr(tc, "id", ""),
            )
        elif etype == "tool_result":
            tr = getattr(event, "tool_result", None) or event.get("tool_result", {})
            return ToolResultEvent(
                tool_call_id=event.get("tool_call_id", "") if isinstance(event, dict) else getattr(event, "tool_call_id", ""),
                output=tr.get("output", "") if isinstance(tr, dict) else getattr(tr, "output", ""),
                success=tr.get("success", True) if isinstance(tr, dict) else getattr(tr, "success", True),
            )
        elif etype == "error":
            msg = getattr(event, "error_message", None) or event.get("error_message", "")
            return ErrorEvent(message=msg)
        elif etype == "done":
            reason = getattr(event, "finish_reason", None) or event.get("finish_reason", "stop")
            return DoneEvent(finish_reason=reason)
        return None
