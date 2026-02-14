"""
Provider Factory

Vercel AI SDK-style provider abstraction for A3S Code.

Example:
    ```python
    from a3s_code import create_provider

    openai = create_provider(name="openai", api_key="sk-xxx")
    kimi = create_provider(name="kimi", api_key="sk-xxx", base_url="http://xxx/v1")

    # Use as model selector
    model = openai("gpt-4o")
    model2 = kimi("k2.5")
    ```
"""

from dataclasses import dataclass
from typing import Optional, Callable


@dataclass
class ModelRef:
    """A resolved provider + model pair."""
    provider: str
    model: str
    api_key: str
    base_url: Optional[str] = None


ModelSelector = Callable[[str], ModelRef]


def create_provider(
    name: str,
    api_key: str,
    base_url: Optional[str] = None,
) -> ModelSelector:
    """Create a provider factory that returns model references.

    Args:
        name: Provider name (e.g., 'openai', 'anthropic', 'kimi')
        api_key: API key
        base_url: Base URL override

    Returns:
        A callable that takes a model ID and returns a ModelRef.

    Example:
        ```python
        openai = create_provider(name="openai", api_key="sk-xxx")
        model = openai("gpt-4o")
        ```
    """
    def selector(model_id: str) -> ModelRef:
        return ModelRef(
            provider=name,
            model=model_id,
            api_key=api_key,
            base_url=base_url,
        )
    return selector
