#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: %s <linux-amd64-a3s-binary> <registry/repository:tag> <output-directory>\n' "$0" >&2
  exit 2
}

if [ "$#" -ne 3 ]; then
  usage
fi

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fixture_directory=$(CDPATH='' cd -- "$script_directory/.." && pwd)
repository_root=$(CDPATH='' cd -- "$fixture_directory/../.." && pwd)
binary_path=$(realpath -- "$1")
image_reference=$2
output_directory=$3

for command_name in cargo docker file jq readelf rustc sha256sum sort; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command_name" >&2
    exit 1
  fi
done

if [ ! -x "$binary_path" ]; then
  printf 'A3S binary is not executable: %s\n' "$binary_path" >&2
  exit 1
fi
if ! file --brief -- "$binary_path" | grep -Eq 'ELF 64-bit LSB.*x86-64'; then
  printf 'fixture requires a Linux amd64 ELF A3S binary\n' >&2
  exit 1
fi
required_glibc=$(readelf --version-info "$binary_path" \
  | sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p' \
  | sort --version-sort \
  | tail -n 1)
if [ -z "$required_glibc" ]; then
  printf 'could not determine the A3S binary GLIBC requirement\n' >&2
  exit 1
fi
newest_glibc=$(printf '%s\n' "$required_glibc" 2.39 \
  | sort --version-sort \
  | tail -n 1)
if [ "$newest_glibc" != 2.39 ]; then
  printf 'A3S binary requires GLIBC %s; fixture base provides at most 2.39\n' \
    "$required_glibc" >&2
  exit 1
fi
case "$image_reference" in
  *@* | *,*)
    printf 'image reference must be one mutable publication tag without digest or comma\n' >&2
    exit 1
    ;;
  */*:*) ;;
  *)
    printf 'image reference must include a registry, repository, and explicit tag\n' >&2
    exit 1
    ;;
esac

mkdir -p -- "$output_directory"
output_directory=$(realpath -- "$output_directory")
manifest_path=$output_directory/.a3s/asset.acl
publication_path=$output_directory/publication.json
builder_provenance_path=$output_directory/provenance/builder.json
for output_path in \
  "$manifest_path" \
  "$publication_path" \
  "$builder_provenance_path"; do
  if [ -e "$output_path" ]; then
    printf 'publication output already exists; refusing to overwrite it: %s\n' \
      "$output_path" >&2
    exit 1
  fi
done

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/a3s-agent-release-publish.XXXXXX")
publication_complete=false
cleanup() {
  if [ "$publication_complete" != true ]; then
    rm -f -- "$manifest_path" "$publication_path" "$builder_provenance_path"
  fi
  case "$temporary_directory" in
    "${TMPDIR:-/tmp}"/a3s-agent-release-publish.*)
      rm -rf -- "$temporary_directory"
      ;;
  esac
}
trap cleanup EXIT

build_context=$temporary_directory/context
mkdir -- "$build_context"
cp -- "$binary_path" "$build_context/a3s"
cp -- "$fixture_directory/.dockerignore" "$build_context/.dockerignore"
cp -- "$fixture_directory/Containerfile" "$build_context/Containerfile"
cp -- "$fixture_directory/config.acl" "$build_context/config.acl"

source_digest=sha256:$(sha256sum -- "$binary_path" | awk '{print $1}')
recipe_inputs=(
  Cargo.lock
  Cargo.toml
  core/Cargo.toml
  core/examples/publish_agent_release_fixture.rs
  core/src/release/error.rs
  core/src/release/manifest.rs
  core/src/release/mod.rs
  core/src/release/schema.rs
  core/src/release/types.rs
  core/src/release/validation.rs
  fixtures/agent-release-contract/.dockerignore
  fixtures/agent-release-contract/.a3s/asset.acl
  fixtures/agent-release-contract/Containerfile
  fixtures/agent-release-contract/config.acl
  fixtures/agent-release-contract/scripts/publish-release.sh
)
recipe_inputs_path=$temporary_directory/recipe-inputs.json
(
  cd -- "$repository_root"
  for input_path in "${recipe_inputs[@]}"; do
    input_digest=sha256:$(sha256sum -- "$input_path" | awk '{print $1}')
    jq -cn \
      --arg path "$input_path" \
      --arg digest "$input_digest" \
      '{path: $path, digest: $digest}'
  done
) | jq -cS -s '.' > "$recipe_inputs_path"
recipe_digest=sha256:$(sha256sum -- "$recipe_inputs_path" | awk '{print $1}')
metadata_path=$temporary_directory/build-metadata.json

docker buildx build \
  --file "$build_context/Containerfile" \
  --platform linux/amd64 \
  --provenance=false \
  --sbom=false \
  --metadata-file "$metadata_path" \
  --output "type=image,name=$image_reference,push=true,oci-mediatypes=true" \
  "$build_context"

artifact_digest=$(jq -er '
  .["containerimage.digest"]
  | select(type == "string")
  | select(test("^sha256:[0-9a-f]{64}$"))
' "$metadata_path")
exact_image_reference=$image_reference@$artifact_digest
media_type=$(docker buildx imagetools inspect "$exact_image_reference" \
  | awk '$1 == "MediaType:" { print $2; exit }')
if [ "$media_type" != 'application/vnd.oci.image.manifest.v1+json' ]; then
  printf 'published artifact is not one OCI image manifest: %s\n' "$media_type" >&2
  exit 1
fi
buildx_version=$(docker buildx version)
cargo_version=$(cargo --version --verbose)
rustc_version=$(rustc --version --verbose)
jq_version=$(jq --version)
builder_provenance_temporary=$temporary_directory/builder-provenance.json
jq -cS -n \
  --arg recipeDigest "$recipe_digest" \
  --slurpfile recipeInputs "$recipe_inputs_path" \
  --arg sourceDigest "$source_digest" \
  --arg artifactDigest "$artifact_digest" \
  --arg artifactMediaType "$media_type" \
  --arg buildxVersion "$buildx_version" \
  --arg cargoVersion "$cargo_version" \
  --arg rustcVersion "$rustc_version" \
  --arg jqVersion "$jq_version" \
  --slurpfile buildMetadata "$metadata_path" \
  '{
    schema: "a3s.code.agent-release-builder-provenance.v1",
    platform: "linux/amd64",
    recipe: {
      digest: $recipeDigest,
      inputs: $recipeInputs[0]
    },
    source: {
      path: "/usr/bin/a3s",
      digest: $sourceDigest
    },
    artifact: {
      digest: $artifactDigest,
      mediaType: $artifactMediaType
    },
    tools: {
      buildx: $buildxVersion,
      cargo: $cargoVersion,
      rustc: $rustcVersion,
      jq: $jqVersion
    },
    buildMetadata: $buildMetadata[0]
  }' > "$builder_provenance_temporary"
builder_digest=sha256:$(sha256sum -- "$builder_provenance_temporary" | awk '{print $1}')

publication_record=$(cargo run \
  --quiet \
  --locked \
  --manifest-path "$repository_root/Cargo.toml" \
  --package a3s-code-core \
  --example publish_agent_release_fixture \
  -- \
  "$fixture_directory/.a3s/asset.acl" \
  "$manifest_path" \
  "$artifact_digest" \
  'urn:a3s:source:a3s-cli-binary' \
  "$source_digest" \
  'urn:a3s:builder:oci-buildkit-v1' \
  "$builder_digest")

publication_temporary=$temporary_directory/publication.json
jq -e \
  --arg exactImageReference "$exact_image_reference" \
  --arg artifactMediaType "$media_type" \
  --arg builderProvenanceDigest "$builder_digest" \
  '. + {
    exactImageReference: $exactImageReference,
    artifactMediaType: $artifactMediaType,
    provenanceArtifacts: [{
      kind: "builder",
      uri: "urn:a3s:builder:oci-buildkit-v1",
      digest: $builderProvenanceDigest,
      mediaType: "application/vnd.a3s.code.agent-release-builder-provenance.v1+json",
      path: "provenance/builder.json"
    }]
  }' \
  <<<"$publication_record" > "$publication_temporary"
mkdir -p -- "$(dirname -- "$builder_provenance_path")"
mv -- "$builder_provenance_temporary" "$builder_provenance_path"
mv -- "$publication_temporary" "$publication_path"
publication_complete=true

printf '%s\n' "$publication_path"
