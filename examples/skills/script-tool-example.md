---
name: script-tool-example
description: Example of script-based tools
version: 1.0.0
tools:
  - name: python-analyze
    description: Analyze data using Python script
    backend:
      type: script
      interpreter: python3
      interpreter_args: []
      script: |
        import json
        import os
        import sys

        # Get arguments from environment
        args = json.loads(os.environ.get('TOOL_ARGS', '{}'))
        data = args.get('data', [])
        operation = args.get('operation', 'sum')

        try:
            numbers = [float(x) for x in data]
            if operation == 'sum':
                result = sum(numbers)
            elif operation == 'avg':
                result = sum(numbers) / len(numbers) if numbers else 0
            elif operation == 'max':
                result = max(numbers) if numbers else None
            elif operation == 'min':
                result = min(numbers) if numbers else None
            else:
                print(f"Unknown operation: {operation}", file=sys.stderr)
                sys.exit(1)

            print(json.dumps({"result": result, "operation": operation}))
        except Exception as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    parameters:
      type: object
      properties:
        data:
          type: array
          items:
            type: number
          description: Array of numbers to analyze
        operation:
          type: string
          enum: ["sum", "avg", "max", "min"]
          description: Operation to perform
      required:
        - data
        - operation

  - name: bash-system-info
    description: Get system information using bash script
    backend:
      type: script
      interpreter: bash
      interpreter_args: ["-e"]
      script: |
        # Get arguments from environment
        INFO_TYPE="${TOOL_ARG_INFO_TYPE:-all}"

        case "$INFO_TYPE" in
          cpu)
            if [[ "$OSTYPE" == "darwin"* ]]; then
              sysctl -n machdep.cpu.brand_string
            else
              cat /proc/cpuinfo | grep "model name" | head -1 | cut -d: -f2
            fi
            ;;
          memory)
            if [[ "$OSTYPE" == "darwin"* ]]; then
              vm_stat | head -5
            else
              free -h
            fi
            ;;
          disk)
            df -h | head -5
            ;;
          all)
            echo "=== System Info ==="
            echo "OS: $(uname -s)"
            echo "Kernel: $(uname -r)"
            echo "Hostname: $(hostname)"
            echo "User: $(whoami)"
            ;;
          *)
            echo "Unknown info type: $INFO_TYPE" >&2
            exit 1
            ;;
        esac
    parameters:
      type: object
      properties:
        info_type:
          type: string
          enum: ["cpu", "memory", "disk", "all"]
          description: Type of system information to retrieve
      required: []

  - name: node-json-transform
    description: Transform JSON data using Node.js
    backend:
      type: script
      interpreter: node
      interpreter_args: []
      script: |
        const args = JSON.parse(process.env.TOOL_ARGS || '{}');
        const data = args.data || {};
        const transform = args.transform || 'identity';

        try {
          let result;
          switch (transform) {
            case 'keys':
              result = Object.keys(data);
              break;
            case 'values':
              result = Object.values(data);
              break;
            case 'entries':
              result = Object.entries(data);
              break;
            case 'flatten':
              result = Array.isArray(data) ? data.flat(Infinity) : data;
              break;
            case 'identity':
            default:
              result = data;
          }
          console.log(JSON.stringify(result, null, 2));
        } catch (e) {
          console.error(`Error: ${e.message}`);
          process.exit(1);
        }
    parameters:
      type: object
      properties:
        data:
          type: object
          description: JSON data to transform
        transform:
          type: string
          enum: ["keys", "values", "entries", "flatten", "identity"]
          description: Transformation to apply
      required:
        - data
---

# Script Tool Examples

Script tools execute inline scripts using various interpreters.

## Features

- **Multiple interpreters**: bash, python, node, ruby, perl, etc.
- **Inline scripts**: Script content embedded in skill definition
- **Environment variables**: Arguments passed as `TOOL_ARG_*` and `TOOL_ARGS` (JSON)
- **Interpreter arguments**: Pass flags to the interpreter (e.g., `-e` for bash)

## Supported Interpreters

| Interpreter | Command | Use Case |
|-------------|---------|----------|
| bash | `bash` | Shell scripts, system commands |
| python3 | `python3` | Data processing, ML tasks |
| node | `node` | JSON manipulation, async tasks |
| ruby | `ruby` | Text processing |
| perl | `perl` | Regex operations |

## Environment Variables

Scripts receive arguments via environment variables:

- `TOOL_ARGS`: Full arguments as JSON string
- `TOOL_ARG_<NAME>`: Individual arguments (uppercase)

Example:
```bash
# For args: {"file_path": "/tmp/test.txt", "count": 10}
# Environment:
#   TOOL_ARGS='{"file_path": "/tmp/test.txt", "count": 10}'
#   TOOL_ARG_FILE_PATH='/tmp/test.txt'
#   TOOL_ARG_COUNT='10'
```
