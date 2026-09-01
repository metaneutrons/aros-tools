#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7080 %s\n' "$*" >&2
    exit 1
}

repository=
tag=
candidate_dir=
while (($#)); do
    case "$1" in
        --repository) repository=${2:-}; shift 2 ;;
        --tag) tag=${2:-}; shift 2 ;;
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        *) fail "unknown release-asset verifier argument: $1" ;;
    esac
done

for command in gh jq python3; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || \
    fail 'repository must be an owner/repository name'
[[ "$tag" =~ ^v[0-9A-Za-z.+-]+$ ]] || fail 'release tag is malformed'
[[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || \
    fail 'candidate-dir must be a real directory'
[[ -n "${GH_TOKEN:-}" ]] || fail 'GH_TOKEN is required for release-asset checks'
[[ -z $(find "$candidate_dir" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'candidate inventory contains a non-regular entry'

script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/aros-release-assets.XXXXXX")
cleanup() {
    rm -rf -- "$work"
}
trap cleanup EXIT
release=$(gh api -H 'X-GitHub-Api-Version: 2026-03-10' \
    "repos/${repository}/releases/tags/${tag}")
release_id=$(jq -er '.id | select(type == "number" and . > 0)' <<<"$release") || \
    fail 'published release has no valid ID'
if [[ $(jq -r '.tag_name' <<<"$release") != "$tag" || \
      $(jq -r '.draft' <<<"$release") != false || \
      $(jq -r '.immutable // false' <<<"$release") != true ]]; then
    fail 'release is not an immutable publication under the exact qualified tag'
fi

gh api -H 'X-GitHub-Api-Version: 2026-03-10' --paginate --slurp \
    "repos/${repository}/releases/${release_id}/assets?per_page=100" \
    > "$work/pages.json"
jq -c '[.[][]]' "$work/pages.json" > "$work/assets.json"
python3 "$script_root/release-asset-metadata.py" validate \
    --version "${tag#v}" --metadata-json "$work/assets.json" \
    --mode exact --candidate-dir "$candidate_dir" >/dev/null || \
    fail 'published release names, bounded sizes or SHA-256 digests differ'

printf '%s\n' "verified exact published asset inventory for $tag"
