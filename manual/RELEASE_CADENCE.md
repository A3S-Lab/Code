# Release cadence

`a3s-code-core` and `a3s-code-acp` ship on a two-week train. A train
leaves on time or not at all. Work that misses it waits for the next one.

## Schedule

- The train day is every second Thursday, starting 2026-10-29.
- The freeze is the day before the train day. After the freeze, only fixes
  for a red gate land on the release candidate.
- The version is the next patch, or a minor when the changelog has an
  `### Added` or a breaking `### Changed` entry.
- A security fix can ship between trains through the same gate.

## Gate

`just release-gate` runs four steps. Every step must pass on the commit
that is uploaded:

1. `scripts/check_release_versions.sh`: every release member has the same
   version.
2. `just ci`: format, clippy `-D warnings`, the workspace tests and the
   landing check.
3. `cargo publish -p a3s-code-core -p a3s-code-acp --dry-run`: Cargo
   verifies ACP against the packaged Core.
4. `just release-smoke`: one ACP turn through the real binary against a
   real provider. The turn must end with `end_turn` and stream a reply.
   - `A3S_RELEASE_SMOKE_CONFIG`: required ACL file with the provider. The
     test fails when it is unset.
   - `A3S_RELEASE_SMOKE_MODEL`: optional `provider/model`.
   - `A3S_RELEASE_SMOKE_BIN`: optional binary. Point it at a clean
     `cargo install --path acp --locked` of the candidate.

When any step is red on the train day, the train is cancelled and the
next one carries the fix. A partial release (Core without ACP) is not
allowed.

## Upload and record

1. Upload Core, then ACP.
2. Date the changelog section.
3. Add the train to the table below, with a link to its evidence.

The Release workflow runs qualification without publishing when dispatched on
main with its default inputs. A stable version tag enables Core/ACP upload and
the matched terminal archive matrix. A dispatch on that tag with `publish=true`
can resume delivery. SDK publication requires the separate `publish_sdks=true`
option; it is not part of the 9.1.4 runtime train.

A train counts as on time when the upload happens on its train day.

| Version | Train day | Uploaded | Evidence |
| --- | --- | --- | --- |
| 9.1.4 | candidate (2026-10-17) | pending, maintainer upload | [`OUTCOME.md`](../../../docs/evidence/code-harness/20261017-release-914-candidate/OUTCOME.md) |
