"""
Provider Configuration Example

Demonstrates how to manage LLM providers and models:
- Add multiple providers (Anthropic, OpenAI, etc.)
- Configure models with costs and limits
- Set default models
- Switch models per session
- List available providers and models
"""

import asyncio
import os
from a3s_code import A3sClient


async def provider_config_example():
    print("=" * 60)
    print("Provider Configuration Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Example 1: Add Anthropic provider
            print("1. Adding Anthropic provider...")
            await client.add_provider(
                name="anthropic",
                api_key=os.getenv("ANTHROPIC_API_KEY", "sk-ant-xxx"),
                base_url="https://api.anthropic.com",
                models=[
                    {
                        "id": "claude-sonnet-4-20250514",
                        "name": "Claude Sonnet 4",
                        "family": "claude-sonnet",
                        "toolCall": True,
                        "temperature": True,
                        "attachment": True,
                        "reasoning": True,
                        "cost": {
                            "input": 3.0,
                            "output": 15.0,
                            "cacheRead": 0.3,
                            "cacheWrite": 3.75
                        },
                        "limit": {
                            "context": 200000,
                            "output": 8192
                        },
                        "modalities": {
                            "input": ["text", "image"],
                            "output": ["text"]
                        }
                    },
                    {
                        "id": "claude-opus-4-20250514",
                        "name": "Claude Opus 4",
                        "family": "claude-opus",
                        "toolCall": True,
                        "temperature": True,
                        "attachment": True,
                        "reasoning": True,
                        "cost": {
                            "input": 15.0,
                            "output": 75.0,
                            "cacheRead": 1.5,
                            "cacheWrite": 18.75
                        },
                        "limit": {
                            "context": 200000,
                            "output": 16384
                        }
                    }
                ]
            )
            print("✓ Anthropic provider added")
            print("  Models: Claude Sonnet 4, Claude Opus 4")
            print()

            # Example 2: Add OpenAI provider
            print("2. Adding OpenAI provider...")
            await client.add_provider(
                name="openai",
                api_key=os.getenv("OPENAI_API_KEY", "sk-xxx"),
                base_url="https://api.openai.com/v1",
                models=[
                    {
                        "id": "gpt-4-turbo",
                        "name": "GPT-4 Turbo",
                        "family": "gpt-4",
                        "toolCall": True,
                        "temperature": True,
                        "cost": {
                            "input": 10.0,
                            "output": 30.0
                        },
                        "limit": {
                            "context": 128000,
                            "output": 4096
                        }
                    },
                    {
                        "id": "gpt-3.5-turbo",
                        "name": "GPT-3.5 Turbo",
                        "family": "gpt-3.5",
                        "toolCall": True,
                        "temperature": True,
                        "cost": {
                            "input": 0.5,
                            "output": 1.5
                        },
                        "limit": {
                            "context": 16385,
                            "output": 4096
                        }
                    }
                ]
            )
            print("✓ OpenAI provider added")
            print("  Models: GPT-4 Turbo, GPT-3.5 Turbo")
            print()

            # Example 3: List all providers
            print("3. Listing all providers...")
            providers = await client.list_providers()
            print(f"✓ Total providers: {len(providers.get('providers', []))}")
            print()

            for provider in providers.get('providers', []):
                print(f"Provider: {provider['name']}")
                print(f"  Base URL: {provider.get('baseUrl', 'N/A')}")
                print(f"  Models: {len(provider.get('models', []))}")

                for model in provider.get('models', []):
                    print(f"    - {model['id']} ({model.get('name', 'N/A')})")
                    limit = model.get('limit', {})
                    cost = model.get('cost', {})
                    print(f"      Context: {limit.get('context', 'N/A')} tokens")
                    print(f"      Output: {limit.get('output', 'N/A')} tokens")
                    print(f"      Cost: ${cost.get('input', 0)}/M input, ${cost.get('output', 0)}/M output")

                    features = []
                    if model.get('toolCall'):
                        features.append('Tool Call')
                    if model.get('temperature'):
                        features.append('Temperature')
                    if model.get('attachment'):
                        features.append('Attachments')
                    if model.get('reasoning'):
                        features.append('Reasoning')

                    if features:
                        print(f"      Features: {', '.join(features)}")
                print()

            # Example 4: Set default model
            print("4. Setting default model...")
            await client.set_default_model("anthropic", "claude-sonnet-4-20250514")
            print("✓ Default model set: anthropic/claude-sonnet-4-20250514")
            print()

            # Example 5: Get default model
            print("5. Getting default model...")
            default_model = await client.get_default_model()
            print("✓ Current default:")
            print(f"  Provider: {default_model.get('provider')}")
            print(f"  Model: {default_model.get('model')}")
            print()

            # Example 6: Create session with specific model
            print("6. Creating session with GPT-4...")
            gpt4_session = await client.create_session(
                name="GPT-4 Session",
                workspace="/tmp/workspace",
                llm={
                    "provider": "openai",
                    "model": "gpt-4-turbo",
                    "apiKey": os.getenv("OPENAI_API_KEY")
                }
            )
            print(f"✓ Session created: {gpt4_session['session_id']}")
            print("  Using: openai/gpt-4-turbo")
            print()

            # Example 7: Create session with Claude
            print("7. Creating session with Claude Sonnet...")
            claude_session = await client.create_session(
                name="Claude Session",
                workspace="/tmp/workspace",
                llm={
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-20250514"
                }
            )
            print(f"✓ Session created: {claude_session['session_id']}")
            print("  Using: anthropic/claude-sonnet-4-20250514")
            print()

            # Example 8: Switch model for existing session
            print("8. Switching model for existing session...")
            await client.configure_session(
                gpt4_session['session_id'],
                llm={
                    "provider": "anthropic",
                    "model": "claude-opus-4-20250514"
                }
            )
            print("✓ Session model switched:")
            print("  From: openai/gpt-4-turbo")
            print("  To: anthropic/claude-opus-4-20250514")
            print()

            # Example 9: Get provider details
            print("9. Getting provider details...")
            anthropic_provider = await client.get_provider("anthropic")
            print("✓ Anthropic provider details:")
            print(f"  Name: {anthropic_provider['name']}")
            print(f"  Base URL: {anthropic_provider.get('baseUrl', 'N/A')}")
            print(f"  Models: {len(anthropic_provider.get('models', []))}")
            print()

            # Example 10: Update provider
            print("10. Updating provider configuration...")
            await client.update_provider(
                "anthropic",
                base_url="https://api.anthropic.com/v1",
                api_key=os.getenv("ANTHROPIC_API_KEY")
            )
            print("✓ Provider updated")
            print()

            # Cleanup
            print("11. Cleanup...")
            await client.destroy_session(gpt4_session['session_id'])
            await client.destroy_session(claude_session['session_id'])
            print("✓ Sessions destroyed")
            print()

            print("=" * 60)
            print("Provider Configuration Example Complete")
            print("=" * 60)
            print()
            print("Key features:")
            print("  ✓ Multiple provider support")
            print("  ✓ Model cost and limit tracking")
            print("  ✓ Per-session model configuration")
            print("  ✓ Runtime model switching")
            print("  ✓ Default model management")

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(provider_config_example())
