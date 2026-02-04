---
name: binary-tool-example
description: Example of binary-based tools
version: 1.0.0
tools:
  - name: jq
    description: Process JSON data using jq command-line tool
    backend:
      type: binary
      path: jq
      args_template: "${filter}"
    parameters:
      type: object
      properties:
        filter:
          type: string
          description: jq filter expression (e.g., ".name", ".[] | select(.age > 18)")
        input:
          type: string
          description: JSON input data
      required:
        - filter
        - input

  - name: custom-binary
    description: Example of downloading and using a custom binary
    backend:
      type: binary
      url: https://example.com/tools/my-tool
      args_template: "process --input ${input_file}"
    parameters:
      type: object
      properties:
        input_file:
          type: string
          description: Path to input file
      required:
        - input_file
---

# Binary Tool Examples

Binary tools execute external binaries installed on the system or downloaded from URLs.

## Features

- **System binaries**: Use tools already installed (e.g., jq, curl, git)
- **Downloaded binaries**: Automatically download and cache from URLs
- **Argument templating**: Use `${arg_name}` to substitute parameters
- **Environment variables**: Pass args as `TOOL_ARG_*` and `TOOL_ARGS` (JSON)

## Usage

```bash
# Install the binary tool (if not already installed)
brew install jq  # or apt-get install jq

# The tool will be available automatically
```
