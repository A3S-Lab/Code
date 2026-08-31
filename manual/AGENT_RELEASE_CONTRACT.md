# Agent Release Contract

## Status and boundary

`a3s-code-core` defines a closed, bounded admission contract for an immutable
Agent release manifest at `.a3s/asset.acl`. The current schema identifier is
`a3s.code.agent-release.v1`.

The `release` module owns:

- parsing untrusted manifest bytes with explicit limits;
- validating the closed v1 schema and semantic constraints;
- deriving a schema-aware canonical ACL document and release identity; and
- checking the declared protocol and capability requirements before activation.

The `agent_protocol` module owns the matching `a3s.code.agent.v1` host
contract. It defines exact release/session/run start, cancellation, and
checkpoint-recovery commands; digest-bound receipts; and bounded cursor pages
that carry the existing lossless `EventEnvelopeV1` values directly. A host may
wrap these values in its own authenticated delivery envelope, but it must not
replace Code's run lifecycle, event names, event sequence, or checkpoint
semantics.

`AgentProtocolHost` is the canonical adapter from that contract into an
`AgentSession`. Exact run IDs are admitted atomically in Code's existing run
store, detached work uses the ordinary Code event and checkpoint pipeline, and
replayed commands never create a second run. The executable service boundary
belongs to `a3s code`; Cloud and node software transport its values and do not
implement another Harness.

The closed `AgentProtocolCommandV1` recovery request intentionally retains its
v1 meaning: load the latest `LoopCheckpoint` stored under a source Run ID. Rust
hosts that already possess portable checkpoint evidence can instead call the
additive `AgentProtocolRunRecoverExactV1` / `execute_exact_recovery()` path.
That request carries the complete `SessionCheckpointDescriptorV1`; Code
validates and pins its exact matching logical boundary under the Session
execution lease before workspace baseline capture or target-Run admission. The
request digest settles the receipt and the descriptor digest is part of the
target Run's immutable replay identity. This adds fail-closed exact-boundary
recovery without adding an enum variant or changing the existing command
endpoint's wire schema.

Live checkpoint capture is a Code-local handoff before that recovery path. A
Rust host may inject `SessionCheckpointExportSink` through `SessionOptions`.
At each completed Tool-round boundary, an internal acknowledgement channel
joins the loop-owned logical state to the event-materialized Session view after
capability effects and prior events settle. The exported Session binding is
the source Run's frozen cognitive authority even if the Session catalog has
already advanced. Code awaits the host sink before the loop continues, but a
sink failure is logged and does not fail the Run. No public event variant,
Cloud checkpoint identity, object authorization, retention rule, approval, or
fork lineage is created; external durability and fencing remain common-Harness
responsibilities.

Every event-page response reads its `RunSnapshot` and retained event window
from one RunStore lock generation. Its state, exclusive sequence bound, and
observation timestamp therefore cannot straddle a concurrent event write.
Run-local logical time is also monotonic after snapshot restore, including new
events, cancellation, and failure, even when the restoring host's wall clock
is behind the persisted value. A response whose requested cursor is at or
beyond Code's exclusive event tail is invalid: the only caught-up non-empty
cursor is `latest_sequence_exclusive - 1`. This makes corrupted or cross-run
consumer cursors fail closed before they can skip later events.

`AgentProtocolHarness` is the matching Code-owned multi-session kernel for that
single executable. One long-lived release process may serve several Cloud
conversations, so it binds each protocol `session_id` to an ordinary
`AgentSession`, resumes a complete configured session-store snapshot before
replay, and retains a finite session set. It does not store runs or events of
its own. Start and recovery may create a missing session; cancellation and
event observation never allocate an unknown conversation. Closing the kernel
closes the same `Agent` and sessions through their existing lifecycle.

For a complete portable artifact,
`AgentProtocolHarness::execute_checkpoint_recovery()` matches the request to
the exact bytes, decodes semantic and logical state together, restores an
unpublished Session, and exposes it in the Harness only after exact target-Run
admission. It performs no snapshot-plus-loop prewrites and never replaces an
unrelated live Session. A persisted semantic generation must match unless the
exact target Run is already present for replay/conflict checking. This is a
process-local Harness visibility guarantee; Cloud/common Harness integration
must provide immutable-object authorization and external store revision/CAS
fencing.

The protocol field `agent_release_identity` is the immutable OCI digest from
`AgentReleaseManifest::artifact().digest()`. It identifies the executable
release that a conforming controller must pin in Runtime. It is intentionally distinct from
`AgentReleaseManifest::identity()`, which identifies the complete canonical ACL
admission document, including health, storage, capability, secret-slot, and
provenance declarations. A native Harness validates both by admitting the
manifest first and then accepting commands only for its declared artifact.

The version-one process transport uses Code-owned paths: commands are posted to
`/v1/agent/commands`, bounded `AgentProtocolEventPageRequestV1` values are
posted to `/v1/agent/events:page`, and exact terminal-run identities are posted
to `/v1/agent/changes`. The last endpoint returns one immutable
`a3s.code.agent-change-set.v1` value: a SHA-256-bound, base64-encoded binary Git
patch between exact `base_tree` and `result_tree` identities, with a 4 MiB raw
patch bound. Hosts must forward the corresponding Code types intact rather
than translating them into another lifecycle or change-set protocol.

For a Git workspace, `AgentProtocolHarness` admits each conversation into a
temporary detached worktree at the clean source repository's `HEAD`. Runs
capture tracked and untracked non-ignored content without mutating the source
index, and the Harness removes the isolated worktree when that conversation is
dropped. A persisted conversation restores its latest captured result tree
when admitted again. Non-Git workspaces use the configured shared path and do
not expose Git-compatible change sets; a dirty Git source fails isolation
admission rather than silently losing host changes.

The Core crate does not build an OCI image, launch the manifest's declared
entrypoint, implement the declared HTTP health endpoints or a network listener,
bind the manifest grace period to a process supervisor, or certify a deployment
as a Runtime Service. The `a3s code` executable and Runtime integration own
those process and transport pieces.

## Native release process

The native version-one process is `a3s code harness`. It admits the release
manifest and verifies the exact Harness protocol and capability surface,
verifies every declared external-secret injection slot, initializes the Agent,
and only then binds the manifest-declared HTTP port. Protocol and capability
failures retain their stable Agent release codes in structured CLI output and
cannot leave a listener behind.

Once active, the process publishes the declared readiness and liveness paths
and the three Code-owned protocol endpoints. Readiness becomes false before
drain. `SIGINT` and `SIGTERM` drain the listener and close Harness sessions
within `health.shutdown_grace_seconds`; exceeding that deadline is a terminal
process failure. Health and protocol-error documents contain neither secret
values nor release identity.

The executable adapter closes the Code-to-process boundary, but it is not OCI
publication or Runtime Service certification. Those require the exact packaged
binary, external manifest injection, registry digest, provenance, Runtime
generation, and retained provider evidence described below.

## Current serve lifecycle building block

With the `serve` feature, `serve::spawn_agent_dir_daemon` provides an observable
lifecycle for the existing filesystem-first cron daemon. Its state progresses
through `starting`, `ready`, `draining`, and `stopped`, or terminates as
`failed`. Readiness is published only after every cron expression is validated
and every enabled schedule session and tool is prepared. Invalid schedules and
tool/session setup failures therefore fail before activation.

`ServeDaemonHandle::stop` cancels sleepers and in-flight schedule turns, closes
daemon-owned sessions, and joins the task within a 30-second default deadline.
Terminal failures expose stable `SERVE_*` codes. Node.js, Python, and Go SDK
serve calls wait for readiness before returning and expose status through their
serve handles.

This is a lifecycle primitive, not the v1 release runtime. It does not read or
bind `.a3s/asset.acl`, listen on the declared readiness/liveness paths, accept a
headless Agent request protocol, launch the declared artifact entrypoint, or
prove Runtime Service deployment. The declared `health.shutdown_grace_seconds`
also remains distinct from the daemon's library default until a release-aware
supervisor owns that mapping.

The checked-in template at
[`fixtures/agent-release-contract/.a3s/asset.acl`](../fixtures/agent-release-contract/.a3s/asset.acl)
is a parser and identity contract fixture. Its repeated hexadecimal digests are
test values, not published OCI or provenance digests, so that file alone is not
a deployable release. The surrounding
[`agent-release-contract`](../fixtures/agent-release-contract/README.md)
directory is a minimal publication recipe: it packages an exact Linux A3S CLI
binary without the final manifest, publishes one OCI image manifest, binds the
resolved artifact and provenance through `AgentReleaseManifest::bind_publication`,
and emits a final canonical ACL beside a secret-free publication record.

Its expected schema-aware identity is
`sha256:d0f1bb153933320102b36703731096ea3030a949f9305a5f9837e7a4ba52e095`.
Cross-repository parser tests may use that value to detect canonicalization
drift, but must not treat it as an artifact digest or deployment certification.

## Publication and certification order

The final manifest cannot be an input to the OCI image whose digest it declares:
that would create a cryptographic self-reference. A conforming producer must use
this order:

1. package the exact `a3s code harness` executable and immutable runtime files,
   without the final release manifest;
2. build and publish the OCI graph once, then resolve its immutable manifest
   digest;
3. generate and admit `.a3s/asset.acl` with that digest and exact provenance;
4. inject the admitted manifest at the declared entrypoint path when creating
   the Runtime Service; and
5. retain evidence that the same digest was pulled, started, observed healthy,
   drained within its deadline, and removed without residual resources.

Certification must additionally prove that external secret values are absent
from the manifest, OCI metadata, process logs, health documents, structured
errors, and retained public evidence. A local parser fixture, mutable image tag,
locally reconstructed digest, mocked HTTP server, or successful compatibility
check is insufficient. Until one retained external-provider run supplies this
evidence, the generated release remains uncertified for that provider even when
its local Docker lifecycle passes.

The controller must treat the final manifest as the source of truth for the
entrypoint, port, readiness/liveness paths, shutdown deadline, writable-storage
profile, and named external-secret targets. Injecting only the artifact digest
into a separately caller-authored generic Service template is insufficient: it
would make the bytes immutable while allowing the release's execution semantics
to drift.

The fixture publisher is invoked from the repository root:

```bash
bash fixtures/agent-release-contract/scripts/publish-release.sh \
  /path/to/linux-amd64/a3s \
  registry.example.com/a3s/code-agent:8.0.3 \
  ./release-output
```

Its local verifier uses an ephemeral registry, pulls the exact digest, confirms
that the final manifest is absent from image metadata/filesystem, injects both
declared external-secret forms, checks health and value-redacting errors, sends
`SIGTERM`, and verifies exit and cleanup:

```bash
bash fixtures/agent-release-contract/scripts/verify-local-release.sh \
  /path/to/linux-amd64/a3s \
  ./release-output
```

This local gate is stronger than a parser or mocked HTTP test, but it is not a
retained registry publication, Cloud Runtime Service run, or real-provider
certification.

## File and schema

A release producer places exactly one `agent_release` block in
`.a3s/asset.acl`:

```acl
agent_release {
  schema = "a3s.code.agent-release.v1"
  protocol = "a3s.code.agent.v1"

  artifact {
    digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    media_type = "application/vnd.oci.image.manifest.v1+json"
  }

  entrypoint {
    command = "/usr/bin/a3s"
    args = ["code", "harness", "--manifest", "/app/.a3s/asset.acl"]
  }

  health {
    transport = "http"
    port = 8080
    readiness_path = "/health/ready"
    liveness_path = "/health/live"
    shutdown_grace_seconds = 30
  }

  storage {
    workspace = "ephemeral"
    cache = "ephemeral"
    persistent_data = "none"
  }

  capability "runtime.service" {
    level = 1
  }

  provenance "source" {
    uri = "https://github.com/A3S-Lab/Code"
    digest = "sha256:2222222222222222222222222222222222222222222222222222222222222222"
  }
}
```

Unknown attributes, blocks, calls, and value shapes fail admission. All strings
below are measured in UTF-8 bytes.

`schema` must be exactly `a3s.code.agent-release.v1`. `protocol` is independently
versioned and uses the canonical form `a3s.code.agent.v<N>`, where `N` is an
integer from 1 through 65,535 with no leading zero. Admission understands that
version syntax; activation still requires an exact match with a protocol the
runtime supplies.

### Artifact and entrypoint

| Field | v1 requirement |
| --- | --- |
| `artifact.digest` | Lowercase `sha256:` plus exactly 64 hexadecimal characters |
| `artifact.media_type` | Exactly `application/vnd.oci.image.manifest.v1+json` |
| `entrypoint.command` | Exactly `/usr/bin/a3s` |
| `entrypoint.args` | Exactly `["code", "harness", "--manifest", "/app/.a3s/asset.acl"]` |

The artifact digest is immutable input to the declared release identity. A tag,
branch, mutable workspace, or registry label is not accepted as an artifact
reference. The fixed entrypoint makes the umbrella `a3s code` command the sole
v1 Harness process; release images cannot substitute a parallel Harness
implementation.

### Health and shutdown declaration

| Field | v1 requirement |
| --- | --- |
| `health.transport` | Exactly `http` |
| `health.port` | Integer from 1 through 65,535 |
| `health.readiness_path` | Canonical absolute path of non-empty ASCII unreserved segments |
| `health.liveness_path` | Same path rules as readiness and different from the readiness path |
| `health.shutdown_grace_seconds` | Integer from 1 through 3,600 |

Path segments may contain ASCII letters, digits, `-`, `.`, `_`, or `~`.
Empty, `.` and `..` segments are rejected.

These fields are declarations consumed by the native Harness process and its
controller. Admission alone does not make either endpoint observable. A
conforming runtime must keep readiness false until it can accept work, report
liveness without exposing release secrets, stop accepting new work during
shutdown, and reach a terminal state within the declared grace period.

### Writable storage boundaries

The v1 modes are deliberately finite:

| Boundary | Accepted values | Meaning |
| --- | --- | --- |
| `storage.workspace` | `read_only`, `ephemeral` | The release may receive a read-only workspace or a release-scoped writable workspace that is discarded |
| `storage.cache` | `none`, `ephemeral` | No cache is mounted, or cache data is disposable |
| `storage.persistent_data` | `none`, `external` | The release owns no durable data, or durable data is supplied and governed outside the image |

The manifest cannot name host paths, volume identifiers, buckets, tenants, or
mutable workspace snapshots. A runtime must reject a deployment it cannot
isolate according to these modes.

### Capabilities

At least one `capability "<name>"` block is required. Names are unique and at
most 128 bytes. Each dot-separated segment starts with a lowercase ASCII letter
and continues with lowercase ASCII letters, digits, or `-`. `level` is an
integer from 1 through 65,535.

Capability blocks form a schema-declared unordered set. Reordering them does
not change canonical bytes or identity. A duplicate name fails admission
instead of choosing one occurrence.

### Secret injection slots

A secret block declares a typed injection slot:

```acl
secret "provider-api-key" {
  target = "environment"
  destination = "PROVIDER_API_KEY"
}

secret "signing-key" {
  target = "file"
  destination = "/run/secrets/signing/key.pem"
}
```

Secret names use the same bounded dotted-name grammar as capabilities. The
accepted targets are:

| Target | Destination |
| --- | --- |
| `environment` | POSIX-style uppercase environment name: `[A-Z_][A-Z0-9_]*`, at most 128 bytes |
| `file` | Canonical path below `/run/secrets/`, at most 256 bytes; each non-empty segment contains only ASCII letters, digits, `-`, `.`, or `_` |

Names and `(target, destination)` pairs must both be unique. Any secret block
also requires a `secrets.external` capability declaration.

The release manifest contains only the slot name and injection destination. It
must never contain a plaintext value, ciphertext, external secret identifier,
vault path, provider resource name, or tenant reference. A deployment
controller resolves an external secret outside the manifest and injects only
runtime material into the declared environment variable or file. The value
must not be copied into artifact metadata, logs, diagnostics, provenance, or
health responses.

The schema is closed, so an attempted `value`, `reference`, or other extra
field fails before canonicalization. Admission errors expose stable codes and
structural fields or indexes without echoing manifest values.

### Provenance

At least one `provenance "<kind>"` block is required. Kinds use the bounded
dotted-name grammar and must be unique. Each reference contains:

- an ASCII `https` URI with a host, or a non-empty ASCII `urn` URI;
- no credentials, query, or fragment; and
- a lowercase SHA-256 digest in the same form as the artifact digest.

The URI is a locator or subject name; the digest binds the immutable provenance
object. Provenance blocks form a schema-declared unordered set.

## Admission budgets

`AgentReleaseManifest::parse` and `AgentReleaseManifest::from_file` apply the
same ACL limits:

| Budget | Limit |
| --- | ---: |
| Document | 64 KiB |
| Nesting depth | 8 |
| Collection items | 256 |
| Token | 8 KiB |
| Diagnostics | 20 |

`from_file` reads at most 64 KiB plus one byte before rejecting an oversized
file. Invalid UTF-8 fails before ACL parsing.

## Canonical identity

After schema and semantic admission, Core computes:

```text
canonical_acl = canonical_bytes_with_schema(document, v1_schema)
identity      = "sha256:" + sha256(canonical_acl)
```

The schema declares `capability`, `secret`, and `provenance` occurrences as
unordered sets. Their source order, comments, whitespace, and ordinary ACL
formatting therefore do not affect identity. Ordered values such as
`entrypoint.args` retain their order. Duplicate set keys are rejected before
identity is returned.

Because `artifact.digest` and all provenance digests are canonical manifest
fields, the identity binds them. The same admitted manifest and artifact digest
produce the same declared identity. Changing the artifact digest, entrypoint,
health declaration, storage boundary, capability, secret slot, or provenance
changes the identity.

Callers comparing complete manifest documents should persist and compare
`AgentReleaseManifest::identity()`, not a digest of the original source
formatting. Protocol commands use the separately documented immutable artifact
digest as `agent_release_identity`.

## Pre-activation compatibility

The runtime supplies an `AgentReleaseCompatibility` containing its exact
protocol and unique available capability levels:

```rust,no_run
use a3s_code_core::release::{
    AgentReleaseCapability, AgentReleaseCompatibility, AgentReleaseManifest,
    AGENT_PROTOCOL_V1,
};

# fn admit(source: &str) -> Result<(), Box<dyn std::error::Error>> {
let release = AgentReleaseManifest::parse(source)?;
let runtime = AgentReleaseCompatibility::new(
    AGENT_PROTOCOL_V1,
    [
        AgentReleaseCapability::new("runtime.service", 1)?,
        AgentReleaseCapability::new("secrets.external", 1)?,
    ],
)?;
release.verify_compatibility(&runtime)?;
# Ok(())
# }
```

Activation fails when the protocol is not an exact match, a required
capability is absent, or the available level is below the required level.
Errors expose stable machine codes:

| Condition | Code |
| --- | --- |
| Protocol mismatch | `a3s.code.agent_release.incompatible_protocol` |
| Missing or insufficient capability | `a3s.code.agent_release.unsupported_capability` |
| Invalid semantic field | `a3s.code.agent_release.invalid_field` |
| Closed-schema rejection | `a3s.code.agent_release.schema` |

Compatibility verification is an admission gate only. A successful result
does not prove artifact availability, secret resolution, storage isolation,
process readiness, or Runtime Service health.

## Versioning and breaking changes

The schema and protocol are versioned independently:

- `schema` governs manifest syntax, admission rules, canonicalization, and
  release identity.
- `protocol` governs the headless Agent request and lifecycle behavior expected
  from the artifact.

Version-one readers never reinterpret a v1 document with new semantics.
Changing any accepted field, mode, limit, required block, set-order rule,
canonical byte rule, or identity meaning requires a new release schema
identifier. This includes adding an otherwise optional field, because old
closed-schema readers would reject it. A new secret target or destination
grammar also requires a new schema version.

A breaking headless request, readiness, liveness, shutdown, or exit-semantics
change requires a new protocol identifier. Runtimes may support multiple
schema or protocol versions explicitly, but they must select by the exact
identifier and fail closed when no supported version matches.

Implementation changes that preserve the complete v1 admission set, typed
meaning, canonical bytes, identity, and protocol behavior do not require a new
identifier. New runtime capability names and higher available capability
levels are also compatible because each release declares and verifies its own
requirements.
