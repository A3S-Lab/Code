import * as path from 'node:path';
import { defineConfig } from '@rspress/core';

const base = process.env.DOCS_BASE ?? '/Code/';
const siteOrigin = process.env.DOCS_ORIGIN ?? 'https://a3s-lab.github.io';

export default defineConfig({
  root: path.join(__dirname, 'docs'),
  base,
  siteOrigin,
  title: 'A3S Code',
  description:
    'A Rust runtime for coding agents with tool calls, approval, event streaming, and recovery. Available for Rust, Node.js, and Python.',
  lang: 'zh',
  icon: '/favicon.svg',
  logo: '/a3s-code-mark.svg',
  logoText: 'A3S Code',
  outDir: 'doc_build',
  llms: true,
  markdown: {
    globalComponents: [
      path.join(__dirname, 'theme/components/AgentBuildingBlocks.tsx'),
      path.join(__dirname, 'theme/components/TuiPlayground.tsx'),
      path.join(__dirname, 'theme/components/WebPlayground.tsx'),
    ],
  },
  multiVersion: {
    default: 'v6',
    versions: ['v6'],
  },
  locales: [
    {
      lang: 'zh',
      label: '简体中文',
      title: 'A3S Code',
      description:
        '用 Rust 构建的编码 Agent 运行时，支持工具调用、权限确认、事件流和任务恢复，并提供 Rust、Node.js、Python API。',
    },
    {
      lang: 'en',
      label: 'English',
      title: 'A3S Code',
      description:
        'A Rust runtime for coding agents with tool calls, approval, event streaming, and recovery. Available for Rust, Node.js, and Python.',
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
