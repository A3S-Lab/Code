# Immutable Agent Release Fixture

This directory contains the version-one A3S Code Agent release publication
fixture. The checked-in [`.a3s/asset.acl`](.a3s/asset.acl) is an admitted parser
template with conspicuous test digests. It is not itself a published release.

The final manifest cannot be copied into the image because it declares that
image's own OCI manifest digest. Publication therefore has one strict order:

1. supply an already-built Linux amd64 `a3s` executable from the matching CLI
   and Code release, requiring no newer than GLIBC 2.39;
2. build and push the image without the final manifest;
3. resolve the pushed OCI image-manifest digest;
4. retain canonical, digest-addressed BuildKit provenance and bind it together
   with the exact binary through `AgentReleaseManifest::bind_publication`; and
5. inject the generated manifest read-only at `/app/.a3s/asset.acl` when the
   digest-pinned image starts.

Publish to an authenticated registry with:

```bash
bash fixtures/agent-release-contract/scripts/publish-release.sh \
  /path/to/linux-amd64/a3s \
  registry.example.com/a3s/code-agent:8.0.3 \
  ./release-output
```

The script refuses existing output, forces one `linux/amd64` OCI image
manifest on a digest-pinned Ubuntu 24.04 base, executes `a3s --version` inside
that image before publication, pushes once, and writes canonical
`release-output/.a3s/asset.acl`, a secret-free `publication.json`, and the exact
canonical `release-output/provenance/builder.json` object whose SHA-256 is the
manifest's `builder` provenance digest. Deploy only the
`exactImageReference` recorded there, never the mutable input tag. The supplied
binary is represented by a digest-bound `source` entry. The retained builder
object records the hashed recipe inputs, Buildx/Cargo/Rust/jq versions, packaged
binary digest, artifact digest, and canonical Buildx metadata; secret values
are never script inputs. That provenance binds and makes auditable the build
that occurred. It does not claim byte-for-byte reproducibility from Ubuntu
package repositories at a later date.

For a hermetic lifecycle check, use a local ephemeral registry:

```bash
bash fixtures/agent-release-contract/scripts/verify-local-release.sh \
  /path/to/linux-amd64/a3s \
  ./release-output
```

That check pulls and starts the exact digest, injects both declared external
secret forms, verifies image metadata and manifest separation, readiness,
liveness, the manifest/publication/provenance digest chain, the packaged binary,
value-redacting protocol errors, SIGTERM shutdown, exit status, and
container/image cleanup. It retains `local-verification.json`, but removes its
ephemeral registry and artifact. This is bounded local Docker evidence, not
retained Cloud Runtime Service certification or proof of an external model
provider.
