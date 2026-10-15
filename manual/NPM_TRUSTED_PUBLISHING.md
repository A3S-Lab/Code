# Code npm publishing

Code publishes its Node SDK from GitHub-hosted runners using npm's OIDC
trusted publishing. The publish job does not use `NPM_TOKEN`. The build jobs
do not receive permission to mint publishing identity tokens.
The calling release job does not forward inherited secrets to this workflow.

## Required npm settings

A package maintainer must add a GitHub Actions trusted publisher on each
package's **Settings → Trusted Publisher** page with these exact values:

| Field | Value |
| --- | --- |
| Organization | `A3S-Lab` |
| Repository | `Code` |
| Workflow filename | `release.yml` |
| Environment | Empty; the Code publish job does not declare an environment |
| Allowed action | Direct publishing with `npm publish` |

The filename is the calling workflow, `release.yml`, even though the publish
command lives in the reusable `publish-node.yml` workflow. Both the caller
job and the publishing job grant `id-token: write`; other release jobs do
not gain this permission.

Apply this binding to all nine packages:

- [@a3s-lab/code](https://www.npmjs.com/package/@a3s-lab/code/access)
- [@a3s-lab/code-darwin-arm64](https://www.npmjs.com/package/@a3s-lab/code-darwin-arm64/access)
- [@a3s-lab/code-darwin-x64](https://www.npmjs.com/package/@a3s-lab/code-darwin-x64/access)
- [@a3s-lab/code-linux-x64-gnu](https://www.npmjs.com/package/@a3s-lab/code-linux-x64-gnu/access)
- [@a3s-lab/code-linux-x64-musl](https://www.npmjs.com/package/@a3s-lab/code-linux-x64-musl/access)
- [@a3s-lab/code-linux-arm64-gnu](https://www.npmjs.com/package/@a3s-lab/code-linux-arm64-gnu/access)
- [@a3s-lab/code-linux-arm64-musl](https://www.npmjs.com/package/@a3s-lab/code-linux-arm64-musl/access)
- [@a3s-lab/code-win32-arm64-msvc](https://www.npmjs.com/package/@a3s-lab/code-win32-arm64-msvc/access)
- [@a3s-lab/code-win32-x64-msvc](https://www.npmjs.com/package/@a3s-lab/code-win32-x64-msvc/access)

If a package has never been published, its maintainer must first create the
package through npm's supported initial-publication flow. Do not add an
ambient token fallback to the release workflow.

## Acceptance

The workflow uses Node 24 and npm 11.5.1, the first npm release that supports
trusted publishing. Publication remains tag-gated and waits for the existing
Code release checks and native builds.

Repository checks cannot prove that npm has accepted a publisher binding.
Acceptance requires a successful Code release run, all nine package versions
present on the public registry, and their npm provenance binding to the exact
Code repository and release commit. Saving a publisher setting alone does
not prove that the binding works.

Code issue [#176](https://github.com/A3S-Lab/Code/issues/176) also tracks an
Office publication failure. Office's separate authorization and release are
owned at [A3S-Lab/Office#162](https://github.com/A3S-Lab/Office/issues/162);
changing Code's workflow does not resolve that incident.

References: [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/)
and [npm provenance](https://docs.npmjs.com/generating-provenance-statements/).
