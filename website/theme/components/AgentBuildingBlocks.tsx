import { useState } from 'react';
import { useLang, useSite, useVersion, withBase } from '@rspress/core/runtime';

type Locale = 'zh' | 'en';

type Localized = {
  zh: string;
  en: string;
};

type BuildingBlock = {
  name: string;
  description: Localized;
  path: string;
};

type BuildingBlockGroup = {
  id: string;
  label: Localized;
  summary: Localized;
  blocks: BuildingBlock[];
};

const groups: BuildingBlockGroup[] = [
  {
    id: 'define',
    label: { zh: '定义 Agent', en: 'Define the agent' },
    summary: {
      zh: '先说明它是谁、要遵守哪些项目规则，以及可以加载哪些配置和 Skill。',
      en: 'Describe the agent, the project rules it follows, and the configuration and Skills it can load.',
    },
    blocks: [
      {
        name: 'AGENTS.md',
        description: {
          zh: '随仓库保存的项目说明、命令和约束。',
          en: 'Repository-owned instructions, commands, and constraints.',
        },
        path: '/guide/agents-md.html',
      },
      {
        name: 'AgentDir',
        description: {
          zh: '用文件组织 Agent、工具和定时任务。',
          en: 'Filesystem definitions for agents, tools, and schedules.',
        },
        path: '/guide/agent-dir.html',
      },
      {
        name: 'A3S ACL',
        description: {
          zh: '配置模型、存储、目录和运行参数。',
          en: 'Configure models, storage, directories, and runtime options.',
        },
        path: '/guide/filesystem-config.html',
      },
      {
        name: 'Skills',
        description: {
          zh: '按需加载可复用的工作说明。',
          en: 'Load reusable task instructions when they are needed.',
        },
        path: '/guide/skills.html',
      },
    ],
  },
  {
    id: 'think',
    label: { zh: '准备上下文', en: 'Prepare context' },
    summary: {
      zh: '选择模型，把项目文件、记忆和其他上下文控制在模型可处理的范围内。',
      en: 'Choose a model and keep project files, memory, and other context within the model window.',
    },
    blocks: [
      {
        name: 'LlmClient',
        description: {
          zh: '连接内置 Provider，或注入自己的模型客户端。',
          en: 'Use a built-in provider or inject your own model client.',
        },
        path: '/guide/providers.html',
      },
      {
        name: 'ContextAssembler',
        description: {
          zh: '挑选、排序并限制送进模型的内容。',
          en: 'Select, rank, and size the content sent to the model.',
        },
        path: '/guide/context.html',
      },
      {
        name: 'MemoryStore',
        description: {
          zh: '保存可以跨会话复用的信息。',
          en: 'Store information that can be reused across sessions.',
        },
        path: '/guide/memory.html',
      },
    ],
  },
  {
    id: 'act',
    label: { zh: '执行操作', en: 'Take action' },
    summary: {
      zh: '通过当前 Workspace 提供文件、命令、Git、网页、MCP 和自定义工具。',
      en: 'Expose files, commands, Git, web, MCP, and custom tools through the current workspace.',
    },
    blocks: [
      {
        name: 'Built-in tools',
        description: {
          zh: '文件、搜索、Shell、Git、Web、Batch 和 Program。',
          en: 'Files, search, shell, Git, web, batch, and program tools.',
        },
        path: '/guide/tools.html',
      },
      {
        name: 'Workspace',
        description: {
          zh: '决定工具能访问什么，以及本地能力是否可用。',
          en: 'Decide what tools can access and which local capabilities exist.',
        },
        path: '/guide/workspace-backends.html',
      },
      {
        name: 'MCP',
        description: {
          zh: '接入外部工具服务。',
          en: 'Connect external tool servers.',
        },
        path: '/guide/mcp.html',
      },
      {
        name: 'Custom tools',
        description: {
          zh: '注册应用自己的工具和结构化结果。',
          en: 'Register application tools and structured results.',
        },
        path: '/guide/tools.html',
      },
    ],
  },
  {
    id: 'control',
    label: { zh: '控制权限', en: 'Control access' },
    summary: {
      zh: '在工具执行前检查权限、询问用户、限制预算，并按需使用沙箱。',
      en: 'Check permissions, ask the user, enforce budgets, and apply a sandbox before tools run.',
    },
    blocks: [
      {
        name: 'PermissionPolicy',
        description: {
          zh: '为工具配置 Allow、Ask 和 Deny 规则。',
          en: 'Configure Allow, Ask, and Deny rules for tools.',
        },
        path: '/guide/security.html',
      },
      {
        name: 'ConfirmationProvider',
        description: {
          zh: '把高风险操作交给用户确认。',
          en: 'Ask the user before a higher-risk operation.',
        },
        path: '/guide/security.html',
      },
      {
        name: 'Hooks & budgets',
        description: {
          zh: '在模型和工具生命周期中加入检查与额度限制。',
          en: 'Add checks and limits to model and tool lifecycles.',
        },
        path: '/guide/hooks.html',
      },
      {
        name: 'Sandbox',
        description: {
          zh: '限制命令能够写入和访问的范围。',
          en: 'Limit what commands can write and access.',
        },
        path: '/guide/isolation.html',
      },
    ],
  },
  {
    id: 'coordinate',
    label: { zh: '拆分任务', en: 'Coordinate work' },
    summary: {
      zh: '把工作交给子 Agent，或用固定流程、队列和定时任务组织执行。',
      en: 'Delegate to child agents or organize work with fixed workflows, queues, and schedules.',
    },
    blocks: [
      {
        name: 'Tasks',
        description: {
          zh: '让模型选择并启动子 Agent。',
          en: 'Let the model select and start child agents.',
        },
        path: '/guide/tasks.html',
      },
      {
        name: 'Teams',
        description: {
          zh: '定义可复用的 Agent 角色。',
          en: 'Define reusable agent roles.',
        },
        path: '/guide/teams.html',
      },
      {
        name: 'Orchestration',
        description: {
          zh: '用 Parallel、Pipeline 和 Checkpoint 编排固定流程。',
          en: 'Build fixed flows with parallel, pipeline, and checkpoints.',
        },
        path: '/guide/orchestration.html',
      },
      {
        name: 'Schedules',
        description: {
          zh: '按计划运行 AgentDir 中的任务。',
          en: 'Run AgentDir tasks on a schedule.',
        },
        path: '/guide/filesystem-schedules.html',
      },
    ],
  },
  {
    id: 'observe',
    label: { zh: '展示与恢复', en: 'Present and recover' },
    summary: {
      zh: '把事件交给 TUI 或 Web 界面，并保存足够的信息来验证、排错和恢复任务。',
      en: 'Feed events to a TUI or web UI and save enough state to verify, debug, and resume work.',
    },
    blocks: [
      {
        name: 'AgentEvent',
        description: {
          zh: '把文本、工具、计划和生命周期变化发送给界面。',
          en: 'Send text, tools, plans, and lifecycle changes to the UI.',
        },
        path: '/guide/sessions.html',
      },
      {
        name: 'Verification',
        description: {
          zh: '保存验证命令、报告和结果。',
          en: 'Save verification commands, reports, and results.',
        },
        path: '/guide/verification.html',
      },
      {
        name: 'Snapshot & checkpoint',
        description: {
          zh: '保存会话和执行进度，之后继续运行。',
          en: 'Save session state and execution progress for later recovery.',
        },
        path: '/guide/persistence.html',
      },
      {
        name: 'A3S TUI / Web',
        description: {
          zh: '把同一套运行事件显示成终端或桌面界面。',
          en: 'Render the same runtime events in terminal or desktop interfaces.',
        },
        path: '/guide/playground/',
      },
    ],
  },
];

const text = {
  zh: {
    aria: '构建 Agent 的组件',
    open: '打开文档',
  },
  en: {
    aria: 'Components for building an agent',
    open: 'Open docs',
  },
};

function localized(value: Localized, locale: Locale) {
  return value[locale];
}

export default function AgentBuildingBlocks() {
  const locale: Locale = useLang() === 'zh' ? 'zh' : 'en';
  const labels = text[locale];
  const [activeId, setActiveId] = useState('act');
  const active = groups.find((group) => group.id === activeId) ?? groups[2];
  const version = useVersion();
  const { site } = useSite();
  const routePrefix = [
    version && version !== site.multiVersion.default ? version : '',
    locale !== site.lang ? locale : '',
  ]
    .filter(Boolean)
    .join('/');
  const route = (pathname: string) => {
    const path = pathname.replace(/^\/+/, '');
    const parts = [routePrefix, path].filter(Boolean).join('/');
    return withBase(`/${parts}`);
  };

  return (
    <section className="a3s-building-blocks" aria-label={labels.aria}>
      <nav className="a3s-building-block-nav" aria-label={labels.aria}>
        {groups.map((group, index) => (
          <button
            aria-pressed={active.id === group.id}
            className={active.id === group.id ? 'is-active' : undefined}
            key={group.id}
            onClick={() => setActiveId(group.id)}
            type="button"
          >
            <span>{String(index + 1).padStart(2, '0')}</span>
            {localized(group.label, locale)}
          </button>
        ))}
      </nav>

      <div className="a3s-building-block-detail">
        <header>
          <span>{localized(active.label, locale)}</span>
          <p>{localized(active.summary, locale)}</p>
        </header>
        <div className="a3s-building-block-grid">
          {active.blocks.map((block) => (
            <a href={route(block.path)} key={`${active.id}-${block.name}`}>
              <strong>{block.name}</strong>
              <span>{localized(block.description, locale)}</span>
              <small>{labels.open} →</small>
            </a>
          ))}
        </div>
      </div>
    </section>
  );
}
