---
name: find-skills
description: Helps users discover and install agent skills when they ask questions like "how do I do X", "find a skill for X", "is there a skill that can...", or express interest in extending capabilities. This skill should be used when the user is looking for functionality that might exist as an installable skill.
---

# Find Skills

This skill helps you discover and install skills from the open agent skills ecosystem using the built-in `search_skills` and `install_skill` tools.

## When to Use This Skill

Use this skill when the user:

- Asks "how do I do X" where X might be a common task with an existing skill
- Says "find a skill for X" or "is there a skill for X"
- Asks "can you do X" where X is a specialized capability
- Expresses interest in extending agent capabilities
- Wants to search for tools, templates, or workflows
- Mentions they wish they had help with a specific domain (design, testing, deployment, etc.)

## Built-in Tools

Three native tools are available for skill discovery and loading:

- **search_skills** - Search the open skills ecosystem (GitHub) by keyword
- **install_skill** - Download and install a skill from a GitHub repository
- **load_skill** - Load a skill's full instructions into the current session by name

**Browse skills at:** https://skills.sh/

## Skill Catalog Mode

When many skills are loaded, only a lightweight catalog of skill names and descriptions is injected into the system prompt instead of full content. Use `load_skill` to load the full instructions for a specific skill on-demand when needed for the current task. This keeps the system prompt concise while still providing access to all available skills.

## How to Help Users Find Skills

### Step 1: Understand What They Need

When a user asks for help with something, identify:

1. The domain (e.g., React, testing, design, deployment)
2. The specific task (e.g., writing tests, creating animations, reviewing PRs)
3. Whether this is a common enough task that a skill likely exists

### Step 2: Search for Skills

Use the `search_skills` tool with a relevant query:

```json
{"query": "react performance"}
```

For example:

- User asks "how do I make my React app faster?" → search_skills(query: "react performance")
- User asks "can you help me with PR reviews?" → search_skills(query: "pr review")
- User asks "I need to create a changelog" → search_skills(query: "changelog")

The tool returns results with skill names, descriptions, star counts, and install commands.

### Step 3: Present Options to the User

When you find relevant skills, present them to the user with:

1. The skill name and what it does
2. The install command (source reference)
3. A link to the GitHub repository

Example response:

```
I found a skill that might help! The "vercel-react-best-practices" skill provides
React and Next.js performance optimization guidelines from Vercel Engineering.

Would you like me to install it?
```

### Step 4: Install the Skill

If the user wants to proceed, use the `install_skill` tool:

```json
{"source": "vercel-labs/agent-skills@vercel-react-best-practices"}
```

For global (user-level) installation:

```json
{"source": "vercel-labs/agent-skills@vercel-react-best-practices", "global": true}
```

**Source format:**
- `owner/repo` - For single-skill repositories (downloads root SKILL.md)
- `owner/repo@skill-name` - For multi-skill repositories (downloads skills/{name}/SKILL.md)

After installation, the skill is **immediately active** in the current session — no restart needed. The skill's instructions are injected into the system prompt so you can use it right away.

## Common Skill Categories

When searching, consider these common categories:

| Category        | Example Queries                          |
| --------------- | ---------------------------------------- |
| Web Development | react, nextjs, typescript, css, tailwind |
| Testing         | testing, jest, playwright, e2e           |
| DevOps          | deploy, docker, kubernetes, ci-cd        |
| Documentation   | docs, readme, changelog, api-docs        |
| Code Quality    | review, lint, refactor, best-practices   |
| Design          | ui, ux, design-system, accessibility     |
| Productivity    | workflow, automation, git                |

## Tips for Effective Searches

1. **Use specific keywords**: "react testing" is better than just "testing"
2. **Try alternative terms**: If "deploy" doesn't work, try "deployment" or "ci-cd"
3. **Check popular sources**: Many skills come from `vercel-labs/agent-skills` or `ComposioHQ/awesome-claude-skills`

## When No Skills Are Found

If no relevant skills exist:

1. Acknowledge that no existing skill was found
2. Offer to help with the task directly using your general capabilities
3. Suggest browsing https://skills.sh/ for more options
4. Suggest the user could create their own skill as a markdown file with YAML frontmatter

Example:

```
I searched for skills related to "xyz" but didn't find any matches.
I can still help you with this task directly! Would you like me to proceed?

You can also browse available skills at: https://skills.sh/
```
