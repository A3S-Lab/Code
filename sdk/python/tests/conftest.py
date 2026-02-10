"""Pytest configuration and fixtures for A3S Code SDK tests."""

import pytest
from a3s_code import A3sClient


@pytest.fixture
def client() -> A3sClient:
    """Create a test client instance."""
    return A3sClient("localhost:50051")


@pytest.fixture
def mock_session_id() -> str:
    """Return a mock session ID for testing."""
    return "test-session-123"
