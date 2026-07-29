import { access, readdir, readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);
const outputRoot = path.join(websiteRoot, 'doc_build');
const base = '/Code/';

const requiredFiles = [
  'index.html',
  'en/index.html',
  'guide/index.html',
  'en/guide/index.html',
  'api/index.html',
  'en/api/index.html',
  'v6.5.1/guide/index.html',
  'v6.5.1/en/guide/index.html',
  'v6.5.0/guide/index.html',
  'v6.5.0/en/guide/index.html',
  'llms.txt',
  'llms-full.txt',
  'en/llms.txt',
  'en/llms-full.txt',
  'a3s-code-mark.svg',
  'social-card.svg',
];

async function collectHtmlFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectHtmlFiles(absolutePath)));
    } else if (entry.name.endsWith('.html')) {
      files.push(absolutePath);
    }
  }

  return files;
}

for (const file of requiredFiles) {
  await access(path.join(outputRoot, file));
}

const capabilityMarkers = [
  'a3s-capability-stories',
  'HITL',
  'PROGRESSIVE API',
  'RUNTIME TOOL',
  'CODE INTELLIGENCE',
  'CTX RECALL',
];

for (const homepage of ['index.html', 'en/index.html']) {
  const html = await readFile(path.join(outputRoot, homepage), 'utf8');
  for (const marker of capabilityMarkers) {
    if (!html.includes(marker)) {
      throw new Error(
        `${homepage} is missing homepage capability marker: ${marker}`,
      );
    }
  }
}

const brokenReferences = [];
const htmlFiles = await collectHtmlFiles(outputRoot);
const referencePattern = /(?:href|src)="([^"]+)"/g;

async function resolvesToBuiltFile(relativeReference) {
  const decodedReference = decodeURIComponent(relativeReference);
  const candidates =
    decodedReference === '' || decodedReference.endsWith('/')
      ? [path.join(decodedReference, 'index.html')]
      : [
          decodedReference,
          `${decodedReference}.html`,
          path.join(decodedReference, 'index.html'),
        ];

  for (const candidate of candidates) {
    const outputPath = path.resolve(outputRoot, candidate);
    if (
      outputPath !== outputRoot &&
      !outputPath.startsWith(`${outputRoot}${path.sep}`)
    ) {
      continue;
    }

    try {
      if ((await stat(outputPath)).isFile()) {
        return true;
      }
    } catch {
      // Try the next supported output form.
    }
  }

  return false;
}

for (const htmlFile of htmlFiles) {
  const html = await readFile(htmlFile, 'utf8');

  for (const [, rawReference] of html.matchAll(referencePattern)) {
    if (
      rawReference.startsWith('#') ||
      rawReference.startsWith('data:') ||
      rawReference.startsWith('mailto:') ||
      /^[a-z]+:\/\//i.test(rawReference)
    ) {
      continue;
    }

    if (rawReference.startsWith('/') && !rawReference.startsWith(base)) {
      brokenReferences.push(
        `${path.relative(outputRoot, htmlFile)} -> ${rawReference} (outside ${base})`,
      );
      continue;
    }

    if (!rawReference.startsWith(base)) {
      continue;
    }

    const withoutBase = rawReference
      .slice(base.length)
      .split(/[?#]/, 1)[0]
      .replace(/\/+/g, '/');
    if (!(await resolvesToBuiltFile(withoutBase))) {
      brokenReferences.push(
        `${path.relative(outputRoot, htmlFile)} -> ${rawReference}`,
      );
    }
  }
}

if (brokenReferences.length > 0) {
  throw new Error(
    `Built-site reference check failed:\n${brokenReferences
      .map((reference) => `  - ${reference}`)
      .join('\n')}`,
  );
}

console.log(
  `Built-site references verified across ${htmlFiles.length} HTML pages.`,
);
