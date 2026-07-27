import { readdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const websiteDirectory = path.resolve(scriptDirectory, '..');
const docsDirectory = path.join(websiteDirectory, 'docs', 'v6');

async function* walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      yield* walk(entryPath);
    } else {
      yield entryPath;
    }
  }
}

function normalizeContent(content) {
  return content
    .replace(
      "import { Tab, Tabs } from 'fumadocs-ui/components/tabs';",
      "import { Tab, Tabs } from '@rspress/core/theme';",
    )
    .replace(
      /<Tabs groupId="lang" items=\{\['Node\.js', 'Python'\]\}>/g,
      '<Tabs>',
    )
    .replace(/<Tab value="([^"]+)">/g, '<Tab label="$1">')
    .replaceAll('/cn/docs/code/', '/guide/')
    .replaceAll('/cn/docs/code)', '/guide/)')
    .replaceAll('/docs/code/', '/guide/')
    .replaceAll('/docs/code)', '/guide/)');
}

let changed = 0;
let removed = 0;

for await (const filePath of walk(docsDirectory)) {
  if (path.basename(filePath) === 'meta.json') {
    await rm(filePath);
    removed += 1;
    continue;
  }

  if (!filePath.endsWith('.md') && !filePath.endsWith('.mdx')) {
    continue;
  }

  const before = await readFile(filePath, 'utf8');
  const after = normalizeContent(before);
  if (after !== before) {
    await writeFile(filePath, after);
    changed += 1;
  }
}

console.log(
  `Normalized ${changed} documentation files and removed ${removed} legacy metadata files.`,
);
