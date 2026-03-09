"""
Dependency Injection Container

Provides a lightweight DI container inspired by NestJS.
Supports three scopes: Singleton, Scoped, and Transient.
"""

import inspect
import uuid
from enum import Enum
from typing import Any, Callable, Dict, Optional, Type, get_type_hints


class Scope(Enum):
    """Provider scope"""

    SINGLETON = "singleton"  # Global singleton
    SCOPED = "scoped"  # Per-session instance
    TRANSIENT = "transient"  # New instance per request


class DIContainer:
    """Dependency Injection Container

    Manages provider registration and dependency resolution.
    Similar to NestJS's DI container.
    """

    def __init__(self):
        self._providers: Dict[Type, dict] = {}
        self._singletons: Dict[Type, Any] = {}
        self._scoped_instances: Dict[str, Dict[Type, Any]] = {}
        self._metadata: Dict[Type, dict] = {}

    def register(
        self, provider_cls: Type, scope: Scope = Scope.SINGLETON, **kwargs
    ) -> None:
        """Register a provider

        Args:
            provider_cls: The provider class to register
            scope: The provider scope (singleton, scoped, transient)
            **kwargs: Constructor arguments
        """
        self._providers[provider_cls] = {
            "class": provider_cls,
            "scope": scope,
            "kwargs": kwargs,
        }
        self._metadata[provider_cls] = {"dependencies": self._get_dependencies(provider_cls)}

    def _get_dependencies(self, cls: Type) -> list:
        """Analyze constructor dependencies

        Uses type hints to determine which parameters are injectable dependencies.
        """
        try:
            sig = inspect.signature(cls.__init__)
            type_hints = get_type_hints(cls.__init__)
        except Exception:
            return []

        dependencies = []
        for param_name, param in sig.parameters.items():
            if param_name == "self":
                continue

            # Get type hint
            param_type = type_hints.get(param_name)
            if param_type and param_type in self._providers:
                dependencies.append(param_type)

        return dependencies

    def resolve(self, cls: Type, scope_id: Optional[str] = None) -> Any:
        """Resolve a provider and its dependencies

        Args:
            cls: The provider class to resolve
            scope_id: The scope ID (required for scoped providers)

        Returns:
            An instance of the provider with all dependencies injected
        """
        if cls not in self._providers:
            raise ValueError(f"Provider {cls.__name__} not registered")

        provider_info = self._providers[cls]
        scope = provider_info["scope"]

        # Singleton: global singleton
        if scope == Scope.SINGLETON:
            if cls not in self._singletons:
                self._singletons[cls] = self._create_instance(cls, scope_id)
            return self._singletons[cls]

        # Scoped: per-scope singleton
        if scope == Scope.SCOPED:
            if not scope_id:
                raise ValueError(
                    f"Scope ID required for scoped provider {cls.__name__}"
                )

            if scope_id not in self._scoped_instances:
                self._scoped_instances[scope_id] = {}

            if cls not in self._scoped_instances[scope_id]:
                self._scoped_instances[scope_id][cls] = self._create_instance(
                    cls, scope_id
                )

            return self._scoped_instances[scope_id][cls]

        # Transient: new instance every time
        return self._create_instance(cls, scope_id)

    def _create_instance(self, cls: Type, scope_id: Optional[str]) -> Any:
        """Create an instance with dependency injection

        Recursively resolves all dependencies and injects them into the constructor.
        """
        provider_info = self._providers[cls]
        dependencies = self._metadata[cls]["dependencies"]

        # Recursively resolve dependencies
        resolved_deps = {}
        for dep_type in dependencies:
            resolved_deps[dep_type] = self.resolve(dep_type, scope_id)

        # Merge constructor kwargs
        kwargs = provider_info["kwargs"].copy()

        # Inject dependencies (match by type)
        try:
            sig = inspect.signature(cls.__init__)
            type_hints = get_type_hints(cls.__init__)

            for param_name, param in sig.parameters.items():
                if param_name == "self":
                    continue

                param_type = type_hints.get(param_name)
                if param_type in resolved_deps:
                    kwargs[param_name] = resolved_deps[param_type]
        except Exception:
            pass

        return cls(**kwargs)

    def clear_scope(self, scope_id: str) -> None:
        """Clear all scoped instances for a given scope ID"""
        if scope_id in self._scoped_instances:
            del self._scoped_instances[scope_id]


# Global container instance
_global_container = DIContainer()


def injectable(scope: str = "singleton"):
    """Decorator to mark a class as injectable

    Args:
        scope: The provider scope (singleton, scoped, transient)

    Example:
        @injectable(scope='singleton')
        class LoggerService:
            def log(self, msg: str):
                print(f"[LOG] {msg}")
    """

    def decorator(cls: Type) -> Type:
        scope_enum = Scope(scope)
        cls._injectable_scope = scope_enum
        return cls

    return decorator


def get_container() -> DIContainer:
    """Get the global DI container"""
    return _global_container
