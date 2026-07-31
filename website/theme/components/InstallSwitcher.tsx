import { useState } from 'react';
import {
  siApple,
  siGo,
  siLinux,
  siNodedotjs,
  siPython,
  siRust,
} from 'simple-icons/icons';

type InstallSwitcherLabels = {
  copied: string;
  copy: string;
  installTabs: string;
};

type BrandIcon = {
  color: string;
  path: string;
  title: string;
};

const windowsIcon: BrandIcon = {
  color: '#4ea4f6',
  path: 'M2 2h9v9H2V2Zm11 0h9v9h-9V2ZM2 13h9v9H2v-9Zm11 0h9v9h-9v-9Z',
  title: 'Windows',
};

const installCommands = [
  {
    id: 'unix',
    label: 'macOS / Linux',
    category: 'CLI',
    packageName: 'a3s code',
    prompt: '$',
    icons: [
      { color: '#f2f4f7', path: siApple.path, title: siApple.title },
      { color: '#f2c94c', path: siLinux.path, title: siLinux.title },
    ],
    commands: [
      "curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.sh | sh",
      'a3s code',
    ],
  },
  {
    id: 'windows',
    label: 'Windows',
    category: 'CLI',
    packageName: 'a3s code',
    prompt: 'PS›',
    icons: [windowsIcon],
    commands: [
      '[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12',
      'irm https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.ps1 | iex',
      'a3s code',
    ],
  },
  {
    id: 'rust',
    label: 'Rust',
    category: 'SDK',
    packageName: 'a3s-code-core',
    prompt: '$',
    icons: [{ color: '#d7dde6', path: siRust.path, title: siRust.title }],
    commands: ['cargo add a3s-code-core'],
  },
  {
    id: 'node',
    label: 'Node.js',
    category: 'SDK',
    packageName: '@a3s-lab/code',
    prompt: '$',
    icons: [
      {
        color: '#68a063',
        path: siNodedotjs.path,
        title: siNodedotjs.title,
      },
    ],
    commands: ['npm install @a3s-lab/code'],
  },
  {
    id: 'python',
    label: 'Python',
    category: 'SDK',
    packageName: 'a3s-code',
    prompt: '$',
    icons: [{ color: '#4b8bbe', path: siPython.path, title: siPython.title }],
    commands: ['python -m pip install a3s-code'],
  },
  {
    id: 'go',
    label: 'Go',
    category: 'SDK',
    packageName: 'sdk/go/v6',
    prompt: '$',
    icons: [{ color: '#56c4dc', path: siGo.path, title: siGo.title }],
    commands: ['go get github.com/A3S-Lab/Code/sdk/go/v6'],
  },
] as const;

function InstallTargetIcon({ icons }: { icons: readonly BrandIcon[] }) {
  return (
    <span className="a3s-install-target-icons" aria-hidden="true">
      {icons.map((icon) => (
        <svg key={icon.title} viewBox="0 0 24 24">
          <path d={icon.path} fill={icon.color} />
        </svg>
      ))}
    </span>
  );
}

export function InstallSwitcher({ labels }: { labels: InstallSwitcherLabels }) {
  const [activeId, setActiveId] =
    useState<(typeof installCommands)[number]['id']>('unix');
  const [copied, setCopied] = useState(false);
  const active =
    installCommands.find((item) => item.id === activeId) ?? installCommands[0];

  async function copyActiveCommand() {
    try {
      await navigator.clipboard.writeText(active.commands.join('\n'));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="a3s-install">
      <div className="a3s-install-console">
        <div
          className="a3s-install-platforms"
          role="tablist"
          aria-label={labels.installTabs}
        >
          {installCommands.map((item, index) => {
            const isActive = active.id === item.id;
            return (
              <button
                aria-controls="a3s-install-panel"
                aria-selected={isActive}
                className={isActive ? 'is-active' : undefined}
                id={`a3s-install-tab-${item.id}`}
                key={item.id}
                onClick={() => {
                  setActiveId(item.id);
                  setCopied(false);
                }}
                onKeyDown={(event) => {
                  let nextIndex = index;
                  if (event.key === 'ArrowRight') nextIndex = index + 1;
                  if (event.key === 'ArrowLeft') nextIndex = index - 1;
                  if (event.key === 'Home') nextIndex = 0;
                  if (event.key === 'End') {
                    nextIndex = installCommands.length - 1;
                  }
                  if (nextIndex === index) return;

                  event.preventDefault();
                  const normalizedIndex =
                    (nextIndex + installCommands.length) %
                    installCommands.length;
                  const nextItem = installCommands[normalizedIndex];
                  setActiveId(nextItem.id);
                  setCopied(false);
                  window.requestAnimationFrame(() => {
                    document
                      .getElementById(`a3s-install-tab-${nextItem.id}`)
                      ?.focus();
                  });
                }}
                role="tab"
                tabIndex={isActive ? 0 : -1}
                type="button"
              >
                <InstallTargetIcon icons={item.icons} />
                <strong>{item.label}</strong>
              </button>
            );
          })}
        </div>

        <div
          aria-labelledby={`a3s-install-tab-${active.id}`}
          className="a3s-install-panel"
          id="a3s-install-panel"
          role="tabpanel"
        >
          <div className="a3s-install-panel-meta">
            <span>
              <strong>{active.packageName}</strong>
              <small>{active.category}</small>
            </span>
            <button
              aria-live="polite"
              className={
                copied ? 'a3s-install-copy is-copied' : 'a3s-install-copy'
              }
              onClick={copyActiveCommand}
              type="button"
            >
              <span aria-hidden="true">{copied ? '✓' : '⧉'}</span>
              {copied ? labels.copied : labels.copy}
            </button>
          </div>
          <div className="a3s-install-code" tabIndex={0}>
            {active.commands.map((command, index) => (
              <div key={`${active.id}-${index}`}>
                <span aria-hidden="true">{active.prompt}</span>
                <code>{command}</code>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
