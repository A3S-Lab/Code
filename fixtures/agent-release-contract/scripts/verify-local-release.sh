#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: %s <linux-amd64-a3s-binary> <output-directory>\n' "$0" >&2
  exit 2
}

if [ "$#" -ne 2 ]; then
  usage
fi

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
binary_path=$(realpath -- "$1")
output_directory=$2

for command_name in cmp curl docker jq realpath sha256sum; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command_name" >&2
    exit 1
  fi
done

mkdir -p -- "$output_directory"
output_directory=$(realpath -- "$output_directory")
manifest_path=$output_directory/.a3s/asset.acl
publication_path=$output_directory/publication.json
builder_provenance_path=$output_directory/provenance/builder.json
evidence_path=$output_directory/local-verification.json
for output_path in \
  "$manifest_path" \
  "$publication_path" \
  "$builder_provenance_path" \
  "$evidence_path"; do
  if [ -e "$output_path" ]; then
    printf 'verification output already exists; refusing to overwrite it: %s\n' \
      "$output_path" >&2
    exit 1
  fi
done

run_token=$$-$(date +%s)
registry_name=a3s-agent-release-registry-$run_token
service_name=a3s-agent-release-service-$run_token
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/a3s-agent-release-verify.XXXXXX")
exact_image_reference=

container_exists() {
  docker container inspect "$1" >/dev/null 2>&1
}

cleanup() {
  if container_exists "$service_name"; then
    docker container rm --force "$service_name" >/dev/null 2>&1 || true
  fi
  if [ -n "$exact_image_reference" ]; then
    docker image rm --force "$exact_image_reference" >/dev/null 2>&1 || true
  fi
  if container_exists "$registry_name"; then
    docker container rm --force "$registry_name" >/dev/null 2>&1 || true
  fi
  case "$temporary_directory" in
    "${TMPDIR:-/tmp}"/a3s-agent-release-verify.*)
      rm -rf -- "$temporary_directory"
      ;;
  esac
}
trap cleanup EXIT

docker run \
  --detach \
  --name "$registry_name" \
  --publish 127.0.0.1::5000 \
  registry:2@sha256:46faa9a1ae6813194b53921a370f2f4f8c5e1aae228a89bceafef5847a6a3278 \
  >/dev/null
registry_binding=$(docker port "$registry_name" 5000/tcp | head -n 1)
registry_port=${registry_binding##*:}
registry_address=127.0.0.1:$registry_port
registry_deadline=$((SECONDS + 10))
while [ "$SECONDS" -lt "$registry_deadline" ]; do
  if curl --connect-timeout 1 --max-time 1 --fail --silent \
    "http://$registry_address/v2/" >/dev/null; then
    break
  fi
  sleep 0.1
done
curl --connect-timeout 1 --max-time 2 --fail --silent --show-error \
  "http://$registry_address/v2/" >/dev/null

image_reference=$registry_address/a3s-code-agent-fixture:$run_token
bash "$script_directory/publish-release.sh" \
  "$binary_path" \
  "$image_reference" \
  "$output_directory" \
  >/dev/null

exact_image_reference=$(jq -er '.exactImageReference' "$publication_path")
artifact_digest=$(jq -er '.artifactDigest' "$publication_path")
manifest_identity=$(jq -er '.manifestIdentity' "$publication_path")
shutdown_grace_seconds=$(jq -er '.health.shutdownGraceSeconds' "$publication_path")
builder_digest=$(jq -er '.provenance[] | select(.kind == "builder") | .digest' \
  "$publication_path")
builder_uri=$(jq -er '.provenance[] | select(.kind == "builder") | .uri' \
  "$publication_path")
source_digest=$(jq -er '.provenance[] | select(.kind == "source") | .digest' \
  "$publication_path")
source_uri=$(jq -er '.provenance[] | select(.kind == "source") | .uri' \
  "$publication_path")

jq -e '
  .schema == "a3s.code.agent-release-publication.v1"
  and (.provenance | type == "array" and length == 2)
  and ([.provenance[].kind] | sort == ["builder", "source"])
  and (all(.provenance[];
    (keys | sort == ["digest", "kind", "uri"])
    and (.digest | test("^sha256:[0-9a-f]{64}$"))))
  and (.provenanceArtifacts | type == "array" and length == 1)
  and (.provenanceArtifacts[0].kind == "builder")
  and (.provenanceArtifacts[0].uri == "urn:a3s:builder:oci-buildkit-v1")
  and (.provenanceArtifacts[0].mediaType
    == "application/vnd.a3s.code.agent-release-builder-provenance.v1+json")
  and (.provenanceArtifacts[0].path == "provenance/builder.json")
' "$publication_path" >/dev/null
if [ "$builder_uri" != 'urn:a3s:builder:oci-buildkit-v1' ] \
  || [ "$source_uri" != 'urn:a3s:source:a3s-cli-binary' ] \
  || [ "$(jq -er '.provenanceArtifacts[0].digest' "$publication_path")" \
    != "$builder_digest" ]; then
  printf 'publication provenance references are inconsistent\n' >&2
  exit 1
fi
if [ ! -f "$builder_provenance_path" ]; then
  printf 'publication omitted its retained builder provenance object\n' >&2
  exit 1
fi
canonical_builder_path=$temporary_directory/canonical-builder-provenance.json
jq -cS . "$builder_provenance_path" > "$canonical_builder_path"
if ! cmp --silent -- "$builder_provenance_path" "$canonical_builder_path"; then
  printf 'builder provenance object is not canonical JSON\n' >&2
  exit 1
fi
retained_builder_digest=sha256:$(sha256sum -- "$builder_provenance_path" \
  | awk '{print $1}')
if [ "$retained_builder_digest" != "$builder_digest" ]; then
  printf 'retained builder provenance digest does not match the manifest\n' >&2
  exit 1
fi
binary_source_digest=sha256:$(sha256sum -- "$binary_path" | awk '{print $1}')
if [ "$binary_source_digest" != "$source_digest" ]; then
  printf 'source provenance does not match the supplied A3S binary\n' >&2
  exit 1
fi
jq -e \
  --arg artifactDigest "$artifact_digest" \
  --arg artifactMediaType "$(jq -er '.artifactMediaType' "$publication_path")" \
  --arg sourceDigest "$source_digest" \
  '
    (keys | sort
      == ["artifact", "buildMetadata", "platform", "recipe", "schema", "source", "tools"])
    and .schema == "a3s.code.agent-release-builder-provenance.v1"
    and .platform == "linux/amd64"
    and (.artifact | keys | sort == ["digest", "mediaType"])
    and .artifact.digest == $artifactDigest
    and .artifact.mediaType == $artifactMediaType
    and (.source | keys | sort == ["digest", "path"])
    and .source.path == "/usr/bin/a3s"
    and .source.digest == $sourceDigest
    and (.recipe | keys | sort == ["digest", "inputs"])
    and (.recipe.digest | test("^sha256:[0-9a-f]{64}$"))
    and (.recipe.inputs | type == "array" and length == 15)
    and (all(.recipe.inputs[];
      (keys | sort == ["digest", "path"])
      and (.digest | test("^sha256:[0-9a-f]{64}$"))
      and (.path | type == "string" and length > 0)
      and (.path | startswith("/") | not)
      and (.path | split("/") | all(. != "" and . != ".."))))
    and (([.recipe.inputs[].path] | length)
      == ([.recipe.inputs[].path] | unique | length))
    and (.tools | keys | sort == ["buildx", "cargo", "jq", "rustc"])
    and (all(.tools[]; type == "string" and length > 0))
    and (.buildMetadata | type == "object")
    and .buildMetadata["containerimage.digest"] == $artifactDigest
  ' "$builder_provenance_path" >/dev/null
recipe_inputs_path=$temporary_directory/recipe-inputs.json
jq -cS '.recipe.inputs' "$builder_provenance_path" > "$recipe_inputs_path"
retained_recipe_digest=sha256:$(sha256sum -- "$recipe_inputs_path" | awk '{print $1}')
if [ "$retained_recipe_digest" \
  != "$(jq -er '.recipe.digest' "$builder_provenance_path")" ]; then
  printf 'builder provenance recipe digest is not self-consistent\n' >&2
  exit 1
fi
manifest_file_identity=sha256:$(sha256sum -- "$manifest_path" | awk '{print $1}')
if [ "$manifest_file_identity" != "$manifest_identity" ]; then
  printf 'canonical manifest bytes do not match the declared release identity\n' >&2
  exit 1
fi
case "$exact_image_reference" in
  *@"$artifact_digest") ;;
  *)
    printf 'exact image reference does not bind the admitted artifact digest\n' >&2
    exit 1
    ;;
esac

docker pull "$exact_image_reference" >/dev/null
image_metadata_path=$temporary_directory/image-metadata.json
docker image inspect "$exact_image_reference" > "$image_metadata_path"
entrypoint=$(jq -c '.[0].Config.Entrypoint' "$image_metadata_path")
if [ "$entrypoint" != '["/usr/bin/a3s","code","harness","--manifest","/app/.a3s/asset.acl"]' ]; then
  printf 'published image changed the admitted Agent entrypoint\n' >&2
  exit 1
fi
docker run --rm --entrypoint /bin/sh "$exact_image_reference" -ec '
  test -x /usr/bin/a3s
  test -f /app/config.acl
  test ! -e /app/.a3s/asset.acl
'
image_source_digest=sha256:$(docker run --rm --entrypoint /usr/bin/sha256sum \
  "$exact_image_reference" /usr/bin/a3s | awk '{print $1}')
if [ "$image_source_digest" != "$source_digest" ]; then
  printf 'packaged A3S binary does not match source provenance\n' >&2
  exit 1
fi

environment_secret=A3S_ENV_SECRET_$run_token
file_secret=A3S_FILE_SECRET_$run_token
secret_path=$temporary_directory/signing-key
printf '%s\n' "$file_secret" > "$secret_path"
chmod 0444 "$secret_path"

docker run \
  --detach \
  --name "$service_name" \
  --env PROVIDER_API_KEY="$environment_secret" \
  --mount "type=bind,src=$manifest_path,dst=/app/.a3s/asset.acl,readonly" \
  --mount "type=bind,src=$secret_path,dst=/run/secrets/signing-key,readonly" \
  "$exact_image_reference" \
  >/dev/null
service_ip=$(docker inspect \
  --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' \
  "$service_name")
if [ -z "$service_ip" ]; then
  printf 'Agent release service has no Docker bridge address\n' >&2
  exit 1
fi
service_address=$service_ip:8080
readiness_path=$temporary_directory/readiness.json
liveness_path=$temporary_directory/liveness.json
readiness_deadline=$((SECONDS + 30))
while [ "$SECONDS" -lt "$readiness_deadline" ]; do
  if ! container_exists "$service_name" || [ "$(docker inspect --format '{{.State.Running}}' "$service_name")" != true ]; then
    docker logs "$service_name" >&2 || true
    printf 'Agent release service exited before readiness\n' >&2
    exit 1
  fi
  if curl --connect-timeout 1 --max-time 1 --fail --silent \
    "http://$service_address/health/ready" > "$readiness_path"; then
    break
  fi
  sleep 0.1
done
curl --connect-timeout 1 --max-time 2 --fail --silent --show-error \
  "http://$service_address/health/ready" > "$readiness_path"
curl --connect-timeout 1 --max-time 2 --fail --silent --show-error \
  "http://$service_address/health/live" > "$liveness_path"
jq -e '.schema == "a3s.code.agent-health.v1" and .status == "ready"' \
  "$readiness_path" >/dev/null
jq -e '.schema == "a3s.code.agent-health.v1" and .status == "live"' \
  "$liveness_path" >/dev/null

protocol_error_path=$temporary_directory/protocol-error.json
protocol_status=$(curl --connect-timeout 1 --max-time 5 --silent --show-error \
  --output "$protocol_error_path" \
  --write-out '%{http_code}' \
  --header 'content-type: application/json' \
  --data "{\"untrusted\":\"$environment_secret\"}" \
  "http://$service_address/v1/agent/commands")
if [ "$protocol_status" != 400 ]; then
  printf 'malformed Agent protocol request returned HTTP %s\n' "$protocol_status" >&2
  exit 1
fi
jq -e '.schema == "a3s.code.agent-error.v1"
  and .error.code == "a3s.code.agent_protocol.invalid_json"' \
  "$protocol_error_path" >/dev/null

logs_path=$temporary_directory/service.log
docker logs "$service_name" > "$logs_path" 2>&1
for retained_path in \
  "$manifest_path" \
  "$publication_path" \
  "$builder_provenance_path" \
  "$readiness_path" \
  "$liveness_path" \
  "$protocol_error_path" \
  "$image_metadata_path" \
  "$logs_path"; do
  if grep -Fq -- "$environment_secret" "$retained_path" \
    || grep -Fq -- "$file_secret" "$retained_path"; then
    printf 'secret value leaked into retained output: %s\n' "$retained_path" >&2
    exit 1
  fi
done

shutdown_started_ns=$(date +%s%N)
docker stop --time "$shutdown_grace_seconds" "$service_name" >/dev/null
shutdown_finished_ns=$(date +%s%N)
shutdown_elapsed_ms=$(((shutdown_finished_ns - shutdown_started_ns) / 1000000))
shutdown_limit_ms=$(((shutdown_grace_seconds + 2) * 1000))
if [ "$shutdown_elapsed_ms" -gt "$shutdown_limit_ms" ]; then
  printf 'Agent release exceeded bounded shutdown: %sms\n' "$shutdown_elapsed_ms" >&2
  exit 1
fi
exit_code=$(docker inspect --format '{{.State.ExitCode}}' "$service_name")
if [ "$exit_code" -ne 0 ]; then
  docker logs "$service_name" >&2 || true
  printf 'Agent release exited with code %s\n' "$exit_code" >&2
  exit 1
fi
docker logs "$service_name" > "$logs_path" 2>&1
if grep -Fq -- "$environment_secret" "$logs_path" \
  || grep -Fq -- "$file_secret" "$logs_path"; then
  printf 'secret value leaked into shutdown logs\n' >&2
  exit 1
fi

evidence_temporary=$temporary_directory/local-verification.json
jq -n \
  --arg artifactDigest "$artifact_digest" \
  --arg manifestIdentity "$manifest_identity" \
  --arg builderProvenanceDigest "$builder_digest" \
  --arg exactImageReference "$exact_image_reference" \
  --argjson readiness "$(jq -c . "$readiness_path")" \
  --argjson liveness "$(jq -c . "$liveness_path")" \
  --argjson shutdownElapsedMs "$shutdown_elapsed_ms" \
  --argjson exitCode "$exit_code" \
  '{
    schema: "a3s.code.agent-release-local-verification.v1",
    runtime: "docker",
    artifactDigest: $artifactDigest,
    manifestIdentity: $manifestIdentity,
    builderProvenanceDigest: $builderProvenanceDigest,
    exactImageReference: $exactImageReference,
    readiness: $readiness,
    liveness: $liveness,
    shutdownElapsedMs: $shutdownElapsedMs,
    exitCode: $exitCode,
    secretValuesRetained: false
  }' > "$evidence_temporary"
mv -- "$evidence_temporary" "$evidence_path"

docker container rm "$service_name" >/dev/null
docker image rm "$exact_image_reference" >/dev/null
docker container rm --force "$registry_name" >/dev/null
exact_image_reference=
if container_exists "$service_name" || container_exists "$registry_name"; then
  printf 'fixture containers remained after verification\n' >&2
  exit 1
fi

printf '%s\n' "$evidence_path"
