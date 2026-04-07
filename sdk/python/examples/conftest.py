"""
pytest configuration and fixtures for a3s-code Python SDK tests.

Provides:
- find_config(): Unified config file location logic
- find_exAMPLES_DIR(): Examples directory path
"""

import os
import sys
from pathlib import Path


def find_EXAMPLES_DIR() -> Path:
    """Get the examples directory path."""
    return Path(__file__).parent.resolve()


def find_config() -> str:
    """
    Unified config file location logic.

    Priority:
    1. A3S_CONFIG environment variable
    2. configs/test_config.hcl in examples directory
    3. ~/.a3s/config.hcl

    Returns:
        str: Path to config file

    Raises:
        FileNotFoundError: If no config file found
    """
    # 1. Environment variable
    if os.environ.get('A3S_CONFIG'):
        config_path = Path(os.environ['A3S_CONFIG'])
        if config_path.exists():
            return str(config_path)

    # 2. Local configs directory
    examples_dir = find_EXAMPLES_DIR()
    local_config = examples_dir / 'configs' / 'test_config.hcl'
    if local_config.exists():
        return str(local_config)

    # 3. Home directory
    home_config = Path.home() / '.a3s' / 'config.hcl'
    if home_config.exists():
        return str(home_config)

    raise FileNotFoundError(
        "No config file found. "
        "Set A3S_CONFIG env var, create configs/test_config.hcl, or create ~/.a3s/config.hcl"
    )


def require_env_vars(*vars: str) -> None:
    """
    Check that required environment variables are set.

    Args:
        *vars: Environment variable names to check

    Raises:
        EnvironmentError: If any variable is not set
    """
    missing = [v for v in vars if not os.environ.get(v)]
    if missing:
        raise EnvironmentError(
            f"Required environment variables not set: {', '.join(missing)}. "
            f"Please set them before running tests."
        )
