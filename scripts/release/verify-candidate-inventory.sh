#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7070 %s\n' "$*" >&2
    exit 1
}

directory=
version=
signed=
while (($#)); do
    case "$1" in
        --directory) directory=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --signed) signed=${2:-}; shift 2 ;;
        *) fail "unknown inventory verifier argument: $1" ;;
    esac
done

[[ -d "$directory" && ! -L "$directory" ]] || fail 'directory is unsafe'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] || \
    fail 'version is malformed'
[[ "$signed" == true || "$signed" == false ]] || fail 'signed must be true or false'
[[ -z $(find "$directory" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'candidate contains a non-regular top-level entry'

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-candidate-inventory.XXXXXX")
cleanup() { find "$work" -type f -delete; rmdir "$work"; }
trap cleanup EXIT
expected="$work/expected"
: > "$expected"
for target in \
    aarch64-apple-darwin aarch64-unknown-linux-gnu \
    x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    archive="aros-tools-v${version}-${target}.tar.gz"
    for name in "$archive" "${archive}.manifest.json" \
        "${archive}.sha256" "aros-tools-v${version}-${target}.spdx.json"; do
        printf '%s\n' "$name" >> "$expected"
        [[ "$signed" == true ]] && printf '%s.sigstore.json\n' "$name" >> "$expected"
    done
done
for arch in amd64 arm64; do
    for name in "aros-tools_${version}_${arch}.deb" \
        "aros-tools_${version}_${arch}.spdx.json"; do
        printf '%s\n' "$name" >> "$expected"
        [[ "$signed" == true ]] && printf '%s.sigstore.json\n' "$name" >> "$expected"
    done
done
for name in PKGBUILD aros-tools.rb; do
    printf '%s\n' "$name" >> "$expected"
    [[ "$signed" == true ]] && printf '%s.sigstore.json\n' "$name" >> "$expected"
done
if [[ "$signed" == true ]]; then
    printf '%s\n' RELEASE_NOTES.md RELEASE_NOTES.md.sigstore.json >> "$expected"
fi
printf '%s\n' SHA256SUMS >> "$expected"
[[ "$signed" == true ]] && printf '%s\n' SHA256SUMS.sigstore.json >> "$expected"
LC_ALL=C sort -o "$expected" "$expected"

actual="$work/actual"
while IFS= read -r -d '' path; do
    printf '%s\n' "${path##*/}"
done < <(find "$directory" -mindepth 1 -maxdepth 1 -type f -print0) | \
    LC_ALL=C sort > "$actual"
if ! diff -u "$expected" "$actual"; then
    fail 'candidate name inventory differs from the closed release contract'
fi

checksummed="$work/checksummed"
awk 'NF == 2 && $1 ~ /^[0-9a-f]{64}$/ && $2 ~ /^(\.\/)?\*?[A-Za-z0-9][A-Za-z0-9._+-]*$/ {
    name = $2; sub(/^\.\//, "", name); sub(/^\*/, "", name); print name
}' "$directory/SHA256SUMS" | LC_ALL=C sort > "$checksummed"
checksum_expected="$work/checksum-expected"
grep -Ev '^SHA256SUMS(\.sigstore\.json)?$' "$expected" > "$checksum_expected"
if ! diff -u "$checksum_expected" "$checksummed"; then
    fail 'SHA256SUMS does not cover the exact pre-checksum staging inventory'
fi

printf '%s\n' "verified closed candidate inventory (${signed}, $version)"
