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
    'A governed coding-agent runtime with explicit tools, policy, events, and durable evidence.',
  lang: 'zh',
  icon: '/favicon.svg',
  logo: '/a3s-code-mark.svg',
  logoText: 'A3S Code',
  outDir: 'doc_build',
  llms: true,
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
        '可治理的编码 Agent 运行时，让工具、策略、事件与持久化证据保持显式。',
    },
    {
      lang: 'en',
      label: 'English',
      title: 'A3S Code',
      description:
        'A governed coding-agent runtime with explicit tools, policy, events, and durable evidence.',
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
