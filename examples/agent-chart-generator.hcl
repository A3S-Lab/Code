# Agent Configuration with Chart Generator Skill
#
# This configuration enables the chart-generator skill for data visualization

agent "data-analyst" {
  # Model configuration
  model = "openai/gpt-4o"

  # Enable skills
  skills {
    # Path to skills directory
    path = "examples/skills"

    # Enable specific skills
    enabled = [
      "chart-generator"  # Data visualization skill
    ]
  }

  # Permission policy
  permissions {
    # Allow read operations for data access
    allow = [
      "read(*)",           # Read any file
      "grep(*)",           # Search file contents
      "glob(*)",           # Find files by pattern
      "bash(cat *)",       # View file contents
      "bash(head *)",      # View file start
      "bash(tail *)",      # View file end
      "bash(wc *)",        # Count lines/words
      "bash(jq *)",        # Parse JSON
      "web_fetch(*)"       # Fetch data from URLs
    ]

    # Deny dangerous operations
    deny = [
      "write(*)",          # No file writes
      "edit(*)",           # No file edits
      "bash(rm *)",        # No deletions
      "bash(mv *)",        # No moves
      "bash(cp *)"         # No copies
    ]

    # Default decision for unlisted operations
    default_decision = "ask"
  }

  # Context configuration
  context {
    # Include project documentation
    include_patterns = [
      "**/*.md",
      "**/*.json",
      "**/*.csv",
      "**/*.txt"
    ]

    # Exclude large files
    exclude_patterns = [
      "**/node_modules/**",
      "**/target/**",
      "**/.git/**"
    ]
  }

  # Memory configuration
  memory {
    enabled = true
    max_entries = 1000
  }
}
