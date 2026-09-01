#!/usr/bin/env bash

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)

fail() {
    printf '::error::AP7225 %s\n' "$*" >&2
    exit 1
}

usage() {
    printf '%s\n' \
        'usage: download-verify-apt-publication.sh --directory DIR' \
        '       --fingerprint HEX --version X.Y.Z|auto' \
        '       [--allow-expired]' \
        '       (--base-url HTTPS_URL | --source-directory DIR)'
}

directory=
fingerprint=
version=
base_url=
source_directory=
allow_expired=false
while (($#)); do
    case "$1" in
        --directory) directory=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --base-url) base_url=${2:-}; shift 2 ;;
        --source-directory) source_directory=${2:-}; shift 2 ;;
        --allow-expired) allow_expired=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail "unknown public APT verifier argument: $1" ;;
    esac
done

[[ -n "$directory" && ! -e "$directory" && ! -L "$directory" ]] || \
    fail 'output directory must be a new path'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'a full 40-hex primary-key fingerprint is required'
[[ "$version" == auto || "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    fail 'version must be canonical SemVer or auto'
if [[ -n "$base_url" && -n "$source_directory" ]] || \
   [[ -z "$base_url" && -z "$source_directory" ]]; then
    fail 'select exactly one public APT source'
fi
if [[ -n "$base_url" ]]; then
    [[ "$base_url" =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?(/[A-Za-z0-9._~-]+)*$ && \
       "$base_url" != */ ]] || fail 'public APT base URL is unsafe'
else
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'source-directory is restricted to release-policy fixtures'
    [[ -d "$source_directory" && ! -L "$source_directory" ]] || \
        fail 'fixture APT source is unsafe'
    source_directory=$(cd "$source_directory" && pwd -P)
fi

for command in awk cmp curl find gpg gpgv gzip install mv sort wc; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done

"$root/scripts/release/prepare-output-parent.sh" --path "$directory" --mode 0755
stage=$(mktemp -d "${directory}.tmp.XXXXXX")
cleanup() {
    trap - EXIT
    rm -rf -- "$stage"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

fetch() {
    local relative=$1 output=$2 class=$3 expected_bytes=${4:-} source
    local arguments=(--output "$output" --class "$class")
    [[ -z "$expected_bytes" ]] || arguments+=(--expected-bytes "$expected_bytes")
    if [[ -n "$source_directory" ]]; then
        source="$source_directory/$relative"
        arguments+=(--source-file "$source")
    else
        arguments+=(--url "$base_url/$relative")
    fi
    AROS_RELEASE_POLICY_FIXTURE="${AROS_RELEASE_POLICY_FIXTURE:-}" \
        "$root/scripts/release/download-bounded-https.sh" "${arguments[@]}" || \
        fail "bounded APT download failed for $relative"
}

fetch aros-tools-archive-keyring.asc "$stage/aros-tools-archive-keyring.asc" apt-key
fetch dists/stable/InRelease "$stage/dists/stable/InRelease" apt-release
fetch dists/stable/Release "$stage/dists/stable/Release" apt-release
fetch dists/stable/Release.gpg "$stage/dists/stable/Release.gpg" apt-signature
clock=()
if [[ -n "${AROS_RELEASE_NOW_EPOCH:-}" ]]; then
    [[ -n "$source_directory" && "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'public APT clock override is restricted to fixtures'
    clock=(--now-epoch "$AROS_RELEASE_NOW_EPOCH")
fi
freshness=()
if [[ "$allow_expired" != true ]]; then
    freshness=(--require-unexpired)
fi
"$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$stage" --mode metadata --fingerprint "$fingerprint" \
    "${freshness[@]}" "${clock[@]}" >/dev/null

release="$stage/dists/stable/Release"
release_index_identity() {
    local wanted=$1
    awk -v wanted="$wanted" '
        $1 == "SHA256:" { section = 1; next }
        section && $0 !~ /^ / { section = 0 }
        section && $3 == wanted { count += 1; digest = $1; size = $2 }
        END {
            if (count != 1) exit 1
            print digest, size
        }
    ' "$release"
}

measured_version=
for arch in amd64 arm64; do
    prefix="dists/stable/main/binary-${arch}"
    for index in Packages Packages.gz; do
        read -r digest size < <(release_index_identity "main/binary-${arch}/${index}") || \
            fail "signed Release has no singular $arch/$index identity"
        [[ "$digest" =~ ^[0-9a-f]{64}$ && "$size" =~ ^[1-9][0-9]*$ ]] || \
            fail "signed Release has malformed $arch/$index identity"
        fetch "$prefix/$index" "$stage/$prefix/$index" apt-index "$size"
        fetch "$prefix/by-hash/SHA256/$digest" \
            "$stage/$prefix/by-hash/SHA256/$digest" apt-index "$size"
    done
    packages="$stage/$prefix/Packages"
    package=$(awk '$1 == "Package:" { print $2 }' "$packages")
    package_version=$(awk '$1 == "Version:" { print $2 }' "$packages")
    package_arch=$(awk '$1 == "Architecture:" { print $2 }' "$packages")
    filename=$(awk '$1 == "Filename:" { print $2 }' "$packages")
    package_size=$(awk '$1 == "Size:" { print $2 }' "$packages")
    [[ "$package" == aros-tools && \
       "$package_version" =~ ^([0-9]+\.[0-9]+\.[0-9]+)-1$ && \
       "$package_arch" == "$arch" && "$package_size" =~ ^[1-9][0-9]*$ ]] || \
        fail "public Packages identity is malformed for $arch"
    current_version=${package_version%-1}
    expected_filename="pool/main/a/aros-tools/aros-tools_${current_version}_${arch}.deb"
    [[ "$filename" == "$expected_filename" ]] || \
        fail "public Packages filename is malformed for $arch"
    if [[ -n "$measured_version" && "$measured_version" != "$current_version" ]]; then
        fail 'public APT architectures disagree on version'
    fi
    measured_version=$current_version
    fetch "$filename" "$stage/$filename" apt-package "$package_size"
done

[[ "$measured_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    fail 'public APT version is missing'
if [[ "$version" != auto && "$version" != "$measured_version" ]]; then
    fail "public APT version is $measured_version; expected $version"
fi
"$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$stage" --mode full --version "$measured_version" \
    --fingerprint "$fingerprint" "${freshness[@]}" "${clock[@]}" >/dev/null

mv "$stage" "$directory"
trap - EXIT HUP INT TERM
printf '%s\n' "$measured_version"
