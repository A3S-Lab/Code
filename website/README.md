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

The published documentation uses exact release revisions. The active
`v6.5.2` content lives under `docs/v6.5.2`; the `v6.5.1` and `v6.5.0`
directories are read-only snapshots extracted from their matching Git tags.
When a release changes the public API, snapshot its documentation under
`docs/<release>` and list the exact revision in `multiVersion.versions` in
`rspress.config.ts`.
