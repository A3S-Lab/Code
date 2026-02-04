---
name: builtin-tools
description: Built-in file operation and shell tools
version: 1.0.0
tools:
  - name: read
    description: Read the contents of a file. Returns line-numbered output. Supports text files and images.
    backend:
      type: binary
      path: a3s-tools
      args_template: "read"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to read (absolute or relative to workspace)
        offset:
          type: integer
          description: Line number to start reading from (0-indexed, default 0)
        limit:
          type: integer
          description: Maximum number of lines to read (default 2000)
      required:
        - file_path

  - name: write
    description: Write content to a file. Creates the file and parent directories if they don't exist.
    backend:
      type: binary
      path: a3s-tools
      args_template: "write"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to write
        content:
          type: string
          description: Content to write to the file
      required:
        - file_path
        - content

  - name: edit
    description: Edit a file by replacing a specific string with another. The old_string must be unique in the file unless replace_all is true.
    backend:
      type: binary
      path: a3s-tools
      args_template: "edit"
    parameters:
      type: object
      properties:
        file_path:
          type: string
          description: Path to the file to edit
        old_string:
          type: string
          description: The exact string to replace (must be unique unless replace_all=true)
        new_string:
          type: string
          description: The string to replace it with
        replace_all:
          type: boolean
          description: Replace all occurrences (default false)
      required:
        - file_path
        - old_string
        - new_string

  - name: bash
    description: Execute a bash command in the workspace directory. Use for running commands, installing packages, running tests, etc.
    backend:
      type: binary
      path: a3s-tools
      args_template: "bash"
    parameters:
      type: object
      properties:
        command:
          type: string
          description: The bash command to execute
        timeout:
          type: integer
          description: Timeout in milliseconds (default 120000)
      required:
        - command

  - name: grep
    description: Search for a pattern in files using ripgrep. Returns matching lines with file paths and line numbers.
    backend:
      type: binary
      path: a3s-tools
      args_template: "grep"
    parameters:
      type: object
      properties:
        pattern:
          type: string
          description: Regular expression pattern to search for
        path:
          type: string
          description: Directory or file to search in (default workspace root)
        glob:
          type: string
          description: Glob pattern to filter files (e.g., '*.rs', '*.{ts,tsx}')
        context:
          type: integer
          description: Number of context lines to show before and after matches
        -i:
          type: boolean
          description: Case insensitive search
      required:
        - pattern

  - name: glob
    description: Find files matching a glob pattern. Returns a list of file paths.
    backend:
      type: binary
      path: a3s-tools
      args_template: "glob"
    parameters:
      type: object
      properties:
        pattern:
          type: string
          description: Glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')
        path:
          type: string
          description: Base directory for the search (default workspace root)
      required:
        - pattern

  - name: ls
    description: List contents of a directory with file types and sizes.
    backend:
      type: binary
      path: a3s-tools
      args_template: "ls"
    parameters:
      type: object
      properties:
        path:
          type: string
          description: Directory path to list (default workspace root)
      required: []
---

# Built-in Tools

Core file operation and shell tools for A3S.

## Tools

- **read**: Read file contents with line numbers
- **write**: Write content to files
- **edit**: Edit files with string replacement
- **bash**: Execute shell commands
- **grep**: Search file contents with ripgrep
- **glob**: Find files by pattern
- **ls**: List directory contents

## Usage

These tools are automatically loaded when A3S starts. They are implemented as a unified binary (`a3s-tools`) with subcommands for each tool.

Parameters are passed via the `TOOL_ARGS` environment variable as JSON, and the workspace is determined from the current directory.
