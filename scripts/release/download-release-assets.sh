#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7082 %s\n' "$*" >&2
    exit 1
}

repository=
release_id=
version=
directory=
mode=
candidate_dir=
while (($#)); do
    case "$1" in
        --repository) repository=${2:-}; shift 2 ;;
        --release-id) release_id=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --directory) directory=${2:-}; shift 2 ;;
        --mode) mode=${2:-}; shift 2 ;;
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        *) fail "unknown bounded release-download argument: $1" ;;
    esac
done

for command_name in curl find gh jq python3 wc; do
    command -v "$command_name" >/dev/null || fail "required command is missing: $command_name"
done
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || \
    fail 'repository must be an owner/repository name'
[[ "$release_id" =~ ^[1-9][0-9]*$ ]] || fail 'release ID must be a positive integer'
[[ "$mode" == exact || "$mode" == subset ]] || fail 'mode must be exact or subset'
[[ -n "$directory" && ! -e "$directory" && ! -L "$directory" ]] || \
    fail 'destination must be one new path'
[[ -n "${GH_TOKEN:-}" ]] || fail 'GH_TOKEN is required for release downloads'
if [[ -n "$candidate_dir" ]]; then
    [[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || \
        fail 'candidate directory is unsafe'
fi

script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
"$script_root/prepare-output-parent.sh" --path "$directory" --mode 0755
stage=$(mktemp -d "${directory}.tmp.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/aros-release-download.XXXXXX")
cleanup() {
    trap - EXIT
    rm -rf -- "$stage" "$work"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

gh api -H 'X-GitHub-Api-Version: 2026-03-10' --paginate --slurp \
    "repos/${repository}/releases/${release_id}/assets?per_page=100" \
    > "$work/pages.json"
jq -c '[.[][]]' "$work/pages.json" > "$work/assets.json"
validation=(python3 "$script_root/release-asset-metadata.py" validate
    --version "$version" --metadata-json "$work/assets.json" --mode "$mode")
[[ -z "$candidate_dir" ]] || validation+=(--candidate-dir "$candidate_dir")
"${validation[@]}" > "$work/assets.tsv"

while IFS=$'\t' read -r asset_id name size digest; do
    [[ -n "$asset_id" ]] || continue
    output="$stage/$name"
    curl --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --max-filesize "$size" \
        --header 'Accept: application/octet-stream' \
        --header 'X-GitHub-Api-Version: 2026-03-10' \
        --header "Authorization: Bearer $GH_TOKEN" \
        "https://api.github.com/repos/${repository}/releases/assets/${asset_id}" \
        --output "$output"
    IFS=$'\t' read -r measured_size measured_digest < <(
        python3 "$script_root/release-asset-metadata.py" identity --file "$output"
    )
    if [[ "$measured_size" != "$size" || "$measured_digest" != "$digest" ]]; then
        fail "downloaded release asset differs from its API identity: $name"
    fi
done < "$work/assets.tsv"

actual=$(find "$stage" -mindepth 1 -maxdepth 1 -type f | wc -l | tr -d ' ')
expected=$(wc -l < "$work/assets.tsv" | tr -d ' ')
[[ "$actual" == "$expected" && \
   -z $(find "$stage" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'downloaded release directory is not one closed regular-file inventory'
mv "$stage" "$directory"
rm -rf -- "$work"
trap - EXIT HUP INT TERM
