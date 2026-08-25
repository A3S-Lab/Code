import * as path from 'node:path';
import { defineConfig } from '@rspress/core';
import type { Theme } from '@code-hike/lighter';
import type { RawCode } from 'codehike/code';
import { remarkCodeHike } from 'codehike/mdx';
import codeHikeTheme from './codehike-theme.json';
import { remarkAclSyntax } from './remark-acl-syntax';

const base = process.env.DOCS_BASE ?? '/Code/';
const siteOrigin = process.env.DOCS_ORIGIN ?? 'https://a3s-lab.github.io';

export default defineConfig({
  root: path.join(__dirname, 'docs'),
  base,
  siteOrigin,
  title: 'A3S Code',
  description:
    'A governed coding-agent runtime with asynchronous workspace retrieval, model-bound evidence, event streaming, and recovery. Available for Rust, Node.js, Python, and Go.',
  lang: 'zh',
  icon: '/favicon.svg',
  logo: '/a3s-code-mark.svg',
  logoText: 'A3S Code',
  outDir: 'doc_build',
  llms: true,
  markdown: {
    remarkPlugins: [
      remarkAclSyntax,
      [
        remarkCodeHike,
        {
          components: { code: 'A3SCodeBlock' },
          ignoreCode: (codeblock: RawCode) => !codeblock.lang,
          syntaxHighlighting: {
            theme: codeHikeTheme as Theme,
          },
        },
      ],
    ],
    globalComponents: [
      path.join(__dirname, 'theme/components/AgentBuildingBlocks.tsx'),
      path.join(__dirname, 'theme/components/A3SCodeBlock.tsx'),
    ],
  },
  multiVersion: {
    default: 'v8.0.0',
    versions: [
      'v8.0.0',
      'v7.0.1',
      'v6.9.0',
      'v6.8.0',
      'v6.7.0',
      'v6.6.0',
      'v6.5.2',
      'v6.5.1',
      'v6.5.0',
    ],
  },
  locales: [
    {
      lang: 'zh',
      label: '简体中文',
      title: 'A3S Code',
      description:
        '用 Rust 构建的受治理编码 Agent 运行时，支持异步工作区检索、模型边界证据、事件流与任务恢复，并提供 Rust、Node.js、Python、Go API。',
    },
    {
      lang: 'en',
      label: 'English',
      title: 'A3S Code',
      description:
        'A governed coding-agent runtime with asynchronous workspace retrieval, model-bound evidence, event streaming, and recovery. Available for Rust, Node.js, Python, and Go.',
    },
  ],
  head: [
    ['meta', { name: 'theme-color', content: '#0b0b0d' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:site_name', content: 'A3S Code' }],
    [
      'meta',
      {
        property: 'og:image',
        content: `${siteOrigin}${base}social-card.svg`,
      },
    ],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    (route) => [
      'link',
      {
        rel: 'canonical',
        href: `${siteOrigin}${base.replace(/\/$/, '')}${route.routePath}`,
      },
    ],
  ],
  themeConfig: {
    darkMode: 'force-dark',
    search: true,
    localeRedirect: 'never',
    enableContentAnimation: true,
    editLink: {
      docRepoBaseUrl: 'https://github.com/A3S-Lab/Code/tree/main/website/docs',
    },
    lastUpdated: {
      author: true,
    },
    llmsUI: {
      placement: 'outline',
      viewOptions: ['markdownLink', 'chatgpt', 'claude'],
    },
    socialLinks: [
      {
        icon: 'github',
        mode: 'link',
        content: 'https://github.com/A3S-Lab/Code',
      },
    ],
  },
});
