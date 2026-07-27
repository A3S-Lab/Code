# A3S Code website

The official multilingual website and documentation for
[A3S Code](https://github.com/A3S-Lab/Code), built with Rspress.

## Local development

```bash
npm ci
npm run dev
```

The production site is served from `/Code/`. Override `DOCS_BASE` and
`DOCS_ORIGIN` only when previewing another deployment target.

## Checks

```bash
npm run format:check
npm run lint
npm run build
```

The published documentation currently contains only the active `v6` line.
When a new major line ships, add a version snapshot under `docs/<version>` and
then list it in `multiVersion.versions` in `rspress.config.ts`.
