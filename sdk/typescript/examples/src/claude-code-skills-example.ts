/**
 * Claude Code Skills Compatibility Example
 *
 * Demonstrates how to use Claude Code skills with A3S Code Agent:
 * - Loading skills with frontmatter (name, description, allowed-tools)
 * - Getting Claude Code skills
 * - Using skills in generation
 */

import { A3sClient } from '@a3s-lab/code';

async function claudeCodeSkillsExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Claude Code Skills Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'claude-skills-demo',
      workspace: '/tmp/claude-skills-test',
      systemPrompt: 'You are a helpful assistant.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Load Claude Code skills
    console.log('2. Loading GitHub commands skill...');
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

    const loadResult = await client.loadSkill(sessionId, 'github-commands', githubSkill);
    console.log(`✓ Loaded skill: ${JSON.stringify(loadResult)}`);
    console.log();

    console.log('3. Loading code review skill...');
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

    await client.loadSkill(sessionId, 'code-review', codeReviewSkill);
    console.log('✓ Loaded code-review skill');
    console.log();

    // Get all Claude Code skills
    console.log('4. Getting all Claude Code skills...');
    const skillsResponse = await client.getClaudeCodeSkills();
    const skills = skillsResponse.skills || [];
    console.log(`✓ Found ${skills.length} Claude Code skills:`);
    for (const skill of skills) {
      console.log(`  - ${skill.name}: ${skill.description}`);
      if (skill.allowedTools) {
        console.log(`    Allowed tools: ${skill.allowedTools}`);
      }
      if (skill.disableModelInvocation) {
        console.log('    Model invocation disabled');
      }
    }
    console.log();

    // Get a specific skill by name
    console.log('5. Getting specific skill...');
    const specific = await client.getClaudeCodeSkills('github-commands');
    const specificSkills = specific.skills || [];
    if (specificSkills.length > 0) {
      console.log('✓ GitHub skill content preview:');
      console.log(`  ${specificSkills[0].content?.substring(0, 100)}...`);
    }
    console.log();

    // Use skill in generation
    console.log('6. Using skill in generation...');
    const response = await client.generate(sessionId, [
      { role: 'ROLE_USER', content: 'List the open issues in this repository' },
    ]);
    if (response.message?.content) {
      console.log(`✓ Response: ${response.message.content.substring(0, 200)}...`);
    }
    console.log();

    // List all skills
    console.log('7. Listing all loaded skills...');
    const allSkills = await client.listSkills(sessionId);
    console.log(`✓ Total skills: ${allSkills.skills?.length || 0}`);
    for (const s of allSkills.skills || []) {
      console.log(`  - ${s.name}: ${(s.description || '').substring(0, 60)}`);
    }
    console.log();

    // Unload skills
    console.log('8. Unloading skills...');
    await client.unloadSkill(sessionId, 'github-commands');
    console.log('✓ Unloaded: github-commands');
    await client.unloadSkill(sessionId, 'code-review');
    console.log('✓ Unloaded: code-review');
    console.log();

    // Clean up
    console.log('9. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Claude Code skills example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

claudeCodeSkillsExample();
