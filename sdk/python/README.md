# A3S Code - Native Python Bindings

Native Python module for the A3S Code AI coding agent, built with PyO3.

```python
from a3s_code import Agent

agent = Agent(model="claude-sonnet-4-20250514", api_key="sk-ant-...", workspace="/project")
result = agent.send("What files handle auth?")
print(result.text)
```

## Installation

```bash
pip install a3s-code
```

## License

MIT
