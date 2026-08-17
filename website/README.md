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

## Documentation version policy

The version selector follows supported API lines, not every package upload.
Its default entry is the newest patch release. A patch that only fixes defects
or clarifies prose updates that current line and records its differences in
`CHANGELOG.md`; it does not need another permanent full-site copy. Preserve an
exact patch snapshot only when callers need its older public tool schema,
configuration, wire behavior, or SDK surface to remain reproducible. Required
parameter or function-signature breaks must use the appropriate minor or major
product version rather than being hidden inside a documentation patch.

The active `v7.0.0` content lives under `docs/v7.0.0`. The `v6.9.0`, `v6.8.0`,
`v6.7.0`, `v6.6.0`, `v6.5.2`, `v6.5.1`, and `v6.5.0` directories remain
read-only historical snapshots; this policy does not rewrite existing
archives. The v6.9 website was published after the package tag, so its exact
source is pinned by the non-release `docs/v6.9.0` tag. For a new minor or major
line, always create a new current directory and archive the previously
supported line. Keep only supported or contract-distinct revisions in the
public selector; release tags and `CHANGELOG.md` retain the complete patch
history.

When archiving a release, list the exact revision in `multiVersion.versions`
in `rspress.config.ts`. Record its tag, source tree, file count, and canonical
SHA-256 in `version-snapshots.json`; never edit that directory afterward.
`npm run lint` verifies the active SDK revision and immutable archive contents.
It also checks repository paths and the Node.js, Python, and Go methods used by
current code examples against the exported SDK source.
