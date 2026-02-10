"""
Integration tests for Python SDK

These tests use the real configuration file from ../../.a3s/config.json
"""

import pytest
from pathlib import Path

from a3s_code.client import (
    load_config_from_file,
    load_config_from_dir,
)

# Get the config path relative to this test file
CONFIG_PATH = Path(__file__).parent.parent.parent.parent / ".a3s" / "config.json"


class TestIntegrationConfig:
    """Integration tests for configuration loading."""

    def test_load_config_from_a3s_dir(self):
        """Test loading config from .a3s/config.json."""
        if not CONFIG_PATH.exists():
            pytest.skip(f"Config file not found at {CONFIG_PATH}")

        config = load_config_from_file(str(CONFIG_PATH))
        assert config is not None
        assert config.get("default_provider") is not None
        assert config.get("default_model") is not None
        assert config.get("providers") is not None
        assert len(config.get("providers", [])) > 0

        print("✓ Loaded config from .a3s/config.json")
        print(f"  Default provider: {config.get('default_provider')}")
        print(f"  Default model: {config.get('default_model')}")
        print(f"  Providers: {len(config.get('providers', []))}")

    def test_default_provider_is_anthropic(self):
        """Test that default provider is anthropic."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        assert config.get("default_provider") == "anthropic"

    def test_providers_have_models(self):
        """Test that providers have models configured."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])

        # Find default provider
        default_provider_name = config.get("default_provider")
        provider = next(
            (p for p in providers if p.get("name") == default_provider_name),
            None,
        )

        assert provider is not None
        assert provider.get("models") is not None
        assert len(provider.get("models", [])) > 0

        print(f"✓ Found provider: {provider.get('name')}")
        print(f"  Models: {len(provider.get('models', []))}")

    def test_default_model_exists_in_provider(self):
        """Test that default model exists in provider."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])
        default_provider_name = config.get("default_provider")
        default_model_id = config.get("default_model")

        provider = next(
            (p for p in providers if p.get("name") == default_provider_name),
            None,
        )
        assert provider is not None

        models = provider.get("models", [])
        model = next(
            (m for m in models if m.get("id") == default_model_id),
            None,
        )

        assert model is not None
        assert model.get("toolCall") is True

        print(f"✓ Found model: {model.get('name')} ({model.get('id')})")
        print(f"  Tool Call: {model.get('toolCall')}")
        print(f"  Reasoning: {model.get('reasoning')}")
        print(f"  Attachment: {model.get('attachment')}")

    def test_api_key_extracted(self):
        """Test that API key is extracted from provider."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        assert config.get("api_key") is not None
        assert config.get("api_key") != ""

        print("✓ API key extracted from provider")

    def test_base_url_configured(self):
        """Test that base URL is configured."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        assert config.get("base_url") is not None

        print(f"✓ Base URL: {config.get('base_url')}")

    def test_list_all_models(self):
        """Test listing all available models."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])

        all_models = []
        for provider in providers:
            for model in provider.get("models", []):
                all_models.append({
                    "provider": provider.get("name"),
                    "model": model,
                })

        assert len(all_models) > 0

        print(f"✓ Available models ({len(all_models)}):")
        for item in all_models:
            model = item["model"]
            print(f"  - {item['provider']}/{model.get('id')}: {model.get('name')} (tool_call: {model.get('toolCall')})")

    def test_model_cost_information(self):
        """Test that model has cost information."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])
        default_provider_name = config.get("default_provider")
        default_model_id = config.get("default_model")

        provider = next(
            (p for p in providers if p.get("name") == default_provider_name),
            None,
        )
        model = next(
            (m for m in provider.get("models", []) if m.get("id") == default_model_id),
            None,
        )

        cost = model.get("cost", {})
        assert cost is not None

        print("✓ Model cost (per million tokens):")
        print(f"  Input: ${cost.get('input')}")
        print(f"  Output: ${cost.get('output')}")
        print(f"  Cache Read: ${cost.get('cacheRead')}")
        print(f"  Cache Write: ${cost.get('cacheWrite')}")

    def test_model_limits(self):
        """Test that model has limits configured."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])
        default_provider_name = config.get("default_provider")
        default_model_id = config.get("default_model")

        provider = next(
            (p for p in providers if p.get("name") == default_provider_name),
            None,
        )
        model = next(
            (m for m in provider.get("models", []) if m.get("id") == default_model_id),
            None,
        )

        limit = model.get("limit", {})
        assert limit is not None

        print("✓ Model limits:")
        print(f"  Context: {limit.get('context')} tokens")
        print(f"  Output: {limit.get('output')} tokens")

    def test_model_modalities(self):
        """Test that model has modalities configured."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])
        default_provider_name = config.get("default_provider")
        default_model_id = config.get("default_model")

        provider = next(
            (p for p in providers if p.get("name") == default_provider_name),
            None,
        )
        model = next(
            (m for m in provider.get("models", []) if m.get("id") == default_model_id),
            None,
        )

        modalities = model.get("modalities", {})
        assert modalities is not None

        print("✓ Model modalities:")
        print(f"  Input: {modalities.get('input')}")
        print(f"  Output: {modalities.get('output')}")

    def test_alternate_provider(self):
        """Test finding alternate provider."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        providers = config.get("providers", [])

        openai_provider = next(
            (p for p in providers if p.get("name") == "openai"),
            None,
        )

        if openai_provider:
            print(f"✓ Found alternate provider: {openai_provider.get('name')}")
            print(f"  Models: {len(openai_provider.get('models', []))}")

            for model in openai_provider.get("models", []):
                print(f"  - {model.get('id')}: {model.get('name')}")
                if model.get("baseUrl"):
                    print(f"    Base URL: {model.get('baseUrl')}")
        else:
            print("  No alternate provider 'openai' configured")

    def test_load_config_from_dir(self):
        """Test loading config from directory."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config_dir = CONFIG_PATH.parent
        config = load_config_from_dir(str(config_dir))

        assert config is not None
        assert config.get("default_provider") is not None

        print(f"✓ Loaded config from directory: {config_dir}")

    def test_address_default(self):
        """Test that address defaults to localhost:4088."""
        if not CONFIG_PATH.exists():
            pytest.skip("Config file not found")

        config = load_config_from_file(str(CONFIG_PATH))
        # Address should default to localhost:4088 if not specified in config
        assert config.get("address") == "localhost:4088"

        print(f"✓ Address: {config.get('address')}")
