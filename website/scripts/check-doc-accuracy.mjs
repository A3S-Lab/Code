import { createHash } from 'node:crypto';
import { access, readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);
const repositoryRoot = path.resolve(websiteRoot, '..');
const docsRoot = path.join(websiteRoot, 'docs');
const errors = [];

function fail(message) {
  errors.push(message);
}

async function exists(file) {
  try {
    await access(file);
    return true;
  } catch {
    return false;
  }
}

async function collectFiles(directory, prefix = '') {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const relativePath = path.posix.join(prefix, entry.name);
    const absolutePath = path.join(directory, entry.name);

    if (entry.isDirectory()) {
      files.push(...(await collectFiles(absolutePath, relativePath)));
    } else {
      files.push(relativePath);
    }
  }

  return files.sort((left, right) =>
    Buffer.from(left).compare(Buffer.from(right)),
  );
}

async function directoryDigest(directory) {
  const files = await collectFiles(directory);
  const hash = createHash('sha256');

  for (const file of files) {
    hash.update(file);
    hash.update('\0');
    const content = await readFile(path.join(directory, file));
    hash.update(
      content.includes(0)
        ? content
        : Buffer.from(content.toString('utf8').replaceAll('\r\n', '\n')),
    );
    hash.update('\0');
  }

  return { files: files.length, sha256: hash.digest('hex') };
}

function extractVersion(content, pattern, source) {
  const match = content.match(pattern);
  if (!match) {
    fail(`Could not read the SDK version from ${source}.`);
    return '';
  }
  return match[1];
}

const manifest = JSON.parse(
  await readFile(path.join(websiteRoot, 'version-snapshots.json'), 'utf8'),
);
const configuredVersions = [
  manifest.current,
  ...manifest.archives.map(({ version }) => version),
];

const [
  coreCargo,
  nodePackage,
  nodeDeclarations,
  pythonProject,
  rspressConfig,
  homeLayout,
  tuiRuntimeDemo,
  tuiWelcomeBanner,
] = await Promise.all([
  readFile(path.join(repositoryRoot, 'core', 'Cargo.toml'), 'utf8'),
  readFile(path.join(repositoryRoot, 'sdk', 'node', 'package.json'), 'utf8'),
  readFile(path.join(repositoryRoot, 'sdk', 'node', 'generated.d.ts'), 'utf8'),
  readFile(
    path.join(repositoryRoot, 'sdk', 'python', 'pyproject.toml'),
    'utf8',
  ),
  readFile(path.join(websiteRoot, 'rspress.config.ts'), 'utf8'),
  readFile(
    path.join(websiteRoot, 'theme', 'components', 'HomeLayout.tsx'),
    'utf8',
  ),
  readFile(
    path.join(websiteRoot, 'theme', 'components', 'TuiRuntimeDemo.tsx'),
    'utf8',
  ),
  readFile(
    path.join(websiteRoot, 'theme', 'components', 'TuiWelcomeBanner.tsx'),
    'utf8',
  ),
]);

const sourceVersions = new Map([
  [
    'core/Cargo.toml',
    extractVersion(
      coreCargo,
      /^\s*version\s*=\s*"([^"]+)"/m,
      'core/Cargo.toml',
    ),
  ],
  ['sdk/node/package.json', JSON.parse(nodePackage).version],
  [
    'sdk/python/pyproject.toml',
    extractVersion(
      pythonProject,
      /^\s*version\s*=\s*"([^"]+)"/m,
      'sdk/python/pyproject.toml',
    ),
  ],
]);
const uniqueSourceVersions = new Set(sourceVersions.values());

if (uniqueSourceVersions.size !== 1) {
  fail(
    `SDK versions differ: ${[...sourceVersions]
      .map(([source, version]) => `${source}=${version}`)
      .join(', ')}.`,
  );
}

const sourceVersion = [...uniqueSourceVersions][0];
const sourceApiLine = sourceVersion?.split('.').slice(0, 2).join('.') ?? '';
const currentApiLine = manifest.current
  .replace(/^v/, '')
  .split('.')
  .slice(0, 2)
  .join('.');
if (currentApiLine !== sourceApiLine) {
  fail(
    `Current docs are ${manifest.current}, but the SDK source API line is v${sourceApiLine}.`,
  );
}

const multiVersionBlock = rspressConfig.match(
  /multiVersion:\s*\{([\s\S]*?)\n\s*\},/,
)?.[1];
const defaultVersion = multiVersionBlock?.match(
  /default:\s*['"]([^'"]+)['"]/,
)?.[1];
const versionsText = multiVersionBlock?.match(/versions:\s*\[([^\]]+)\]/)?.[1];
const versions = versionsText
  ? [...versionsText.matchAll(/['"]([^'"]+)['"]/g)].map((match) => match[1])
  : [];

if (defaultVersion !== manifest.current) {
  fail(
    `Rspress default revision is ${defaultVersion ?? 'missing'}, expected ${manifest.current}.`,
  );
}
if (JSON.stringify(versions) !== JSON.stringify(configuredVersions)) {
  fail(
    `Rspress revisions are [${versions.join(', ')}], expected [${configuredVersions.join(', ')}].`,
  );
}
if (
  ![homeLayout, tuiRuntimeDemo, tuiWelcomeBanner].some((source) =>
    source.includes(`a3s-code v${sourceVersion}`),
  )
) {
  fail(`The home page does not display a3s-code v${sourceVersion}.`);
}

for (const version of configuredVersions) {
  if (!(await exists(path.join(docsRoot, version)))) {
    fail(`Missing documentation revision directory: docs/${version}.`);
  }
}

for (const archive of manifest.archives) {
  const archiveRoot = path.join(docsRoot, archive.version);
  if (!(await exists(archiveRoot))) {
    continue;
  }

  const digest = await directoryDigest(archiveRoot);
  if (digest.files !== archive.files) {
    fail(
      `${archive.version} has ${digest.files} files, expected ${archive.files} from ${archive.sourceTag}.`,
    );
  }
  if (digest.sha256 !== archive.sha256) {
    fail(
      `${archive.version} no longer matches its ${archive.sourceTag} snapshot (SHA-256 ${digest.sha256}).`,
    );
  }
}

const currentRoot = path.join(docsRoot, manifest.current);
const currentFiles = (await collectFiles(currentRoot)).filter((file) =>
  /\.(?:md|mdx)$/.test(file),
);
const currentDocuments = new Map(
  await Promise.all(
    currentFiles.map(async (file) => [
      file,
      await readFile(path.join(currentRoot, file), 'utf8'),
    ]),
  ),
);

const sdkTabs = new Map([
  ['Rust', /```rust(?:[^\n]*)\n/],
  ['Node.js', /```(?:ts|typescript)(?:[^\n]*)\n/],
  ['Python', /```python(?:[^\n]*)\n/],
  ['Go', /```go(?:[^\n]*)\n/],
]);
let aclFenceCount = 0;

function looksLikeAcl(source) {
  return (
    /(?:^|\n)\s*(?:version|provider|model|agent_dirs|skill_dirs|mcp_servers|memory|session)\s*=/m.test(
      source,
    ) ||
    /(?:^|\n)\s*(?:provider|model|agent|mcp|tool)\s+"[^"]+"\s*\{/m.test(source)
  );
}

for (const [file, content] of currentDocuments) {
  if (file.startsWith('en/') && /[\u3400-\u9fff]/u.test(content)) {
    fail(`${manifest.current}/${file} contains Chinese text.`);
  }

  if (file.startsWith('zh/')) {
    const rawTitle = content.match(/^title:\s*(.+)$/m)?.[1]?.trim() ?? '';
    const title = rawTitle.replace(/^(['"])(.*)\1$/, '$2');
    const allowedTechnicalTitles = new Set([
      'A3S Code',
      'A3S Code TUI',
      'AGENTS.md',
      'MCP',
      'agent.acl',
      'instructions.md',
    ]);
    if (
      title &&
      !/[\u3400-\u9fff]/u.test(title) &&
      !allowedTechnicalTitles.has(title)
    ) {
      fail(`${manifest.current}/${file} has an untranslated title: ${title}.`);
    }
  }

  for (const staleText of [
    'crates/code/',
    '/guide/go-sdk',
    'Node/Python wrappers will follow',
    'Node/Python 封装稍后',
    'not yet exposed on the JS/Python option surface',
    '尚未暴露在 JS/Python 选项面上',
    'This feature is Rust-side',
    '该特性位于 Rust 侧',
  ]) {
    if (content.includes(staleText)) {
      fail(`${manifest.current}/${file} contains stale text: ${staleText}.`);
    }
  }

  for (const [index, match] of [
    ...content.matchAll(/<Tabs>([\s\S]*?)<\/Tabs>/g),
  ].entries()) {
    const tabs = match[1];
    if (
      ![...sdkTabs.keys()].some((label) => tabs.includes(`label="${label}"`))
    ) {
      continue;
    }

    for (const [label, codeFence] of sdkTabs) {
      const tab = tabs.match(
        new RegExp(`<Tab label="${label}">([\\s\\S]*?)<\\/Tab>`),
      )?.[1];
      if (!tab) {
        fail(
          `${manifest.current}/${file} SDK tab group ${index + 1} is missing ${label}.`,
        );
      } else if (!codeFence.test(tab)) {
        fail(
          `${manifest.current}/${file} SDK tab group ${index + 1} has no complete ${label} code block.`,
        );
      }
    }
  }

  aclFenceCount += [...content.matchAll(/^```acl(?:[^\n]*)\n/gm)].length;
  for (const block of content.matchAll(
    /^```(?:text|txt|hcl)(?:[^\n]*)\n([\s\S]*?)^```/gm,
  )) {
    if (looksLikeAcl(block[1])) {
      fail(
        `${manifest.current}/${file} contains ACL configuration in a non-ACL code fence.`,
      );
    }
  }

  for (const match of content.matchAll(
    /`((?:core|scripts|sdk|website)\/[A-Za-z0-9._@+/-]+)`/g,
  )) {
    const reference = match[1];
    if (reference.includes('X.Y.Z')) {
      continue;
    }
    if (
      reference.includes('..') ||
      !(await exists(path.resolve(repositoryRoot, reference)))
    ) {
      fail(
        `${manifest.current}/${file} references a missing repository path: ${reference}.`,
      );
    }
  }
}

if (aclFenceCount === 0) {
  fail(`${manifest.current} contains no ACL code fences.`);
}

const aclRemarkPath = path.join(websiteRoot, 'remark-acl-syntax.ts');
if (!(await exists(aclRemarkPath))) {
  fail('Missing the ACL syntax-highlighting remark plugin.');
} else {
  const aclRemarkSource = await readFile(aclRemarkPath, 'utf8');
  if (
    !rspressConfig.includes('remarkAclSyntax') ||
    !aclRemarkSource.includes("node.lang = 'hcl'") ||
    !aclRemarkSource.includes('displayLanguage=ACL')
  ) {
    fail('ACL fences are not mapped to the HCL grammar with an ACL label.');
  }
}

for (const language of ['en', 'zh']) {
  if (await exists(path.join(currentRoot, language, 'guide', 'go-sdk.mdx'))) {
    fail(
      `${manifest.current}/${language}/guide/go-sdk.mdx is standalone; Go belongs in the shared SDK chapters.`,
    );
  }
}

const sharedGoPages = [
  'guide/examples/quick-start.mdx',
  'guide/examples/streaming.mdx',
  'guide/examples/direct-tools.mdx',
  'guide/sessions.mdx',
  'guide/verification.mdx',
  'guide/mcp.mdx',
  'guide/persistence.mdx',
  'guide/telemetry.mdx',
];
for (const language of ['en', 'zh']) {
  for (const page of sharedGoPages) {
    const relativePath = `${language}/${page}`;
    if (!currentDocuments.get(relativePath)?.includes('```go')) {
      fail(`${manifest.current}/${relativePath} is missing its Go example.`);
    }
  }
}

const goFiles = (await collectFiles(path.join(repositoryRoot, 'sdk', 'go')))
  .filter((file) => file.endsWith('.go'))
  .filter((file) => !file.endsWith('_test.go'));
const goMethods = {
  Agent: new Set(),
  Session: new Set(),
};

for (const file of goFiles) {
  const content = await readFile(
    path.join(repositoryRoot, 'sdk', 'go', file),
    'utf8',
  );
  for (const match of content.matchAll(
    /func\s+\(\s*\w+\s+\*(Agent|Session)\s*\)\s+([A-Z][A-Za-z0-9_]*)\s*\(/g,
  )) {
    goMethods[match[1]].add(match[2]);
  }
}

function nodeClassMethods(className, nextClassName) {
  const startMarker = `export declare class ${className} {`;
  const start = nodeDeclarations.indexOf(startMarker);
  const end = nextClassName
    ? nodeDeclarations.indexOf(`export declare class ${nextClassName} {`, start)
    : nodeDeclarations.length;
  const methods = new Set();

  if (start < 0 || end < 0) {
    fail(`Could not inspect Node.js ${className} declarations.`);
    return methods;
  }

  const classBody = nodeDeclarations.slice(start, end);
  for (const match of classBody.matchAll(
    /^\s{2}(?:static\s+)?([A-Za-z][A-Za-z0-9_]*)\s*(?:<[^>\n]+>)?\s*\(/gm,
  )) {
    if (match[1] !== 'constructor') methods.add(match[1]);
  }
  return methods;
}

const nodeMethods = {
  Agent: nodeClassMethods('Agent'),
  Session: nodeClassMethods('Session', 'Agent'),
};

const pythonAgentSource = await readFile(
  path.join(repositoryRoot, 'sdk', 'python', 'src', 'agent.rs'),
  'utf8',
);
const pythonSessionSources = await Promise.all(
  [
    'session.rs',
    'session_capabilities.rs',
    'session_memory.rs',
    'session_queue_api.rs',
    'session_tools.rs',
    'workspace_retrieval.rs',
  ].map((file) =>
    readFile(path.join(repositoryRoot, 'sdk', 'python', 'src', file), 'utf8'),
  ),
);

function pythonMethods(source) {
  return new Set(
    [
      ...source.matchAll(
        /^\s+(?:pub\(super\)\s+)?fn\s+([a-z][a-z0-9_]*)(?:<[^>\n]+>)?\s*\(/gm,
      ),
    ]
      .map((match) => match[1])
      .filter((method) => !method.startsWith('__')),
  );
}

const pythonMethodsByReceiver = {
  Agent: pythonMethods(
    pythonAgentSource.slice(pythonAgentSource.indexOf('impl PyAgent {')),
  ),
  Session: pythonMethods(pythonSessionSources.join('\n')),
};

function normalizedMethodName(method) {
  return method.replaceAll('_', '').toLowerCase();
}

const nodeOnlySessionConvenienceMethods = new Set([
  // Go methods already accept a context and block until completion; these
  // Promise aliases do not represent separate runtime capabilities.
  'cancelAsync',
  'closeAsync',
]);
const pythonSessionCapabilities = new Set(
  [...pythonMethodsByReceiver.Session].flatMap((method) => {
    const normalized = normalizedMethodName(method);
    return normalized.endsWith('async')
      ? [normalized, normalized.slice(0, -'async'.length)]
      : [normalized];
  }),
);
const goSessionCapabilities = new Set(
  [...goMethods.Session].map(normalizedMethodName),
);

for (const method of nodeMethods.Session) {
  if (nodeOnlySessionConvenienceMethods.has(method)) {
    continue;
  }
  const capability = normalizedMethodName(method);
  if (!pythonSessionCapabilities.has(capability)) {
    fail(
      `Python Session is missing the Node.js ${method}() capability (${capability}).`,
    );
  }
  if (!goSessionCapabilities.has(capability)) {
    fail(
      `Go Session is missing the Node.js ${method}() capability (${capability}).`,
    );
  }
}

const rustAgentApiFiles = (
  await collectFiles(path.join(repositoryRoot, 'core', 'src', 'agent_api'))
).filter((file) => file.endsWith('.rs'));
const rustAgentFacade = await readFile(
  path.join(repositoryRoot, 'core', 'src', 'agent_api', 'agent_facade.rs'),
  'utf8',
);
const rustSessionSources = await Promise.all(
  rustAgentApiFiles
    .filter((file) => file !== 'agent_facade.rs')
    .map((file) =>
      readFile(
        path.join(repositoryRoot, 'core', 'src', 'agent_api', file),
        'utf8',
      ),
    ),
);

function rustMethods(source) {
  return new Set(
    [
      ...source.matchAll(
        /\bpub\s+(?:async\s+)?fn\s+([a-z][a-z0-9_]*)\s*(?:<[^>{}\n]*>)?\s*\(/g,
      ),
    ].map((match) => match[1]),
  );
}

const rustMethodsByReceiver = {
  Agent: rustMethods(rustAgentFacade),
  Session: rustMethods(rustSessionSources.join('\n')),
};

for (const [file, content] of currentDocuments) {
  for (const block of content.matchAll(/```rust[^\n]*\n([\s\S]*?)```/g)) {
    for (const call of block[1].matchAll(
      /\b(agent|session)\.([a-z][a-z0-9_]*)\s*\(/g,
    )) {
      const receiver = call[1] === 'agent' ? 'Agent' : 'Session';
      if (!rustMethodsByReceiver[receiver].has(call[2])) {
        fail(
          `${manifest.current}/${file} calls missing Rust ${receiver} method ${call[2]}().`,
        );
      }
    }
  }

  for (const block of content.matchAll(/```go[^\n]*\n([\s\S]*?)```/g)) {
    for (const call of block[1].matchAll(
      /\b(agent|session)\.([A-Z][A-Za-z0-9_]*)\s*\(/g,
    )) {
      const receiver = call[1] === 'agent' ? 'Agent' : 'Session';
      if (!goMethods[receiver].has(call[2])) {
        fail(
          `${manifest.current}/${file} calls missing Go ${receiver} method ${call[2]}().`,
        );
      }
    }
  }

  for (const block of content.matchAll(
    /```(?:ts|typescript)[^\n]*\n([\s\S]*?)```/g,
  )) {
    for (const call of block[1].matchAll(
      /\b(agent|session)\.([A-Za-z][A-Za-z0-9_]*)\s*\(/g,
    )) {
      const receiver = call[1] === 'agent' ? 'Agent' : 'Session';
      if (!nodeMethods[receiver].has(call[2])) {
        fail(
          `${manifest.current}/${file} calls missing Node.js ${receiver} method ${call[2]}().`,
        );
      }
    }
  }

  for (const block of content.matchAll(/```python[^\n]*\n([\s\S]*?)```/g)) {
    for (const call of block[1].matchAll(
      /\b(agent|session)\.([a-z][a-z0-9_]*)\s*\(/g,
    )) {
      const receiver = call[1] === 'agent' ? 'Agent' : 'Session';
      if (!pythonMethodsByReceiver[receiver].has(call[2])) {
        fail(
          `${manifest.current}/${file} calls missing Python ${receiver} method ${call[2]}().`,
        );
      }
    }
  }
}

if (errors.length > 0) {
  throw new Error(
    `Documentation accuracy check failed:\n${errors
      .map((error) => `  - ${error}`)
      .join('\n')}`,
  );
}

console.log(
  `Documentation accuracy verified for ${manifest.current}, ${manifest.archives.length} immutable revision snapshots, ACL highlighting, and Rust/Node.js/Python/Go SDK examples.`,
);
