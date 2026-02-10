/**
 * Claude Code Skills Compatibility Example
 *
 * Demonstrates how to use Claude Code skills with A3S Code Agent.
 * Claude Code skills are prompt-based skills with optional tool permissions.
 */

import { CodeAgentClient } from "../src";

async function main() {
  const client = new CodeAgentClient();

  try {
    // Connect to the agent
    await client.connect();

    // Initialize the agent
    await client.initialize({ workspace: "/tmp/claude-skills-demo" });

    // Create a session
    const session = await client.createSession();
    console.log(`Created session: ${session.sessionId}`);

    // Load a Claude Code skill (GitHub commands)
    const githubSkill = `---
name: github-commands
description: GitHub CLI commands for repository management
allowed-tools: Bash(gh:*)
---

Use the \`gh\` CLI for all GitHub operations:

1. For issues: \`gh issue list\`, \`gh issue view <number>\`
2. For PRs: \`gh pr list\`, \`gh pr view <number>\`, \`gh pr create\`
3. For repos: \`gh repo view\`, \`gh repo clone\`

Always prefer \`gh\` over direct API calls or web scraping.
`;

    const loadResult = await client.loadSkill({
      sessionId: session.sessionId,
      skillName: "github-commands",
      skillContent: githubSkill,
    });
    console.log(`Loaded skill with tools: ${loadResult.toolNames.join(", ")}`);

    // Load a code review skill (Claude Code format)
    const codeReviewSkill = `---
name: code-review
description: Code review a pull request
allowed-tools: Bash(gh issue view:*), Bash(gh pr:*), Read(*)
disable-model-invocation: false
---

Provide a code review for the given pull request.

Steps:
1. Check if the PR is open and not a draft
2. Read the PR diff using \`gh pr diff\`
3. Review for bugs, style issues, and CLAUDE.md compliance
4. Comment on the PR with findings
`;

    await client.loadSkill({
      sessionId: session.sessionId,
      skillName: "code-review",
      skillContent: codeReviewSkill,
    });
    console.log("Loaded code-review skill");

    // Get all Claude Code skills
    const skillsResponse = await client.getClaudeCodeSkills({});
    console.log(`\nLoaded Claude Code skills (${skillsResponse.skills.length}):`);
    for (const skill of skillsResponse.skills) {
      console.log(`  - ${skill.name}: ${skill.description}`);
      if (skill.allowedTools) {
        console.log(`    Allowed tools: ${skill.allowedTools}`);
      }
      if (skill.disableModelInvocation) {
        console.log(`    Model invocation disabled`);
      }
    }

    // Get a specific skill by name
    const specificSkill = await client.getClaudeCodeSkills({
      name: "github-commands",
    });
    if (specificSkill.skills.length > 0) {
      const skill = specificSkill.skills[0];
      console.log(`\nGitHub skill content preview:`);
      console.log(`  ${skill.content.substring(0, 100)}...`);
    }

    // Use the skill in a generation request
    const response = await client.generate({
      sessionId: session.sessionId,
      prompt: "List the open issues in this repository",
    });
    console.log(`\nGeneration response: ${response.content.substring(0, 200)}...`);

    // Clean up
    await client.destroySession({ sessionId: session.sessionId });
    console.log("\nSession destroyed");
  } finally {
    await client.disconnect();
  }
}

main().catch(console.error);
