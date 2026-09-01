#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7050 %s\n' "$*" >&2
    exit 1
}

output_dir=
while (($#)); do
    case "$1" in
        --output-dir) output_dir=${2:-}; shift 2 ;;
        *) fail "unknown pinned-Syft installer argument: $1" ;;
    esac
done

for command in curl python3 tar; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done
if command -v sha256sum >/dev/null; then
    checksum=(sha256sum)
elif command -v shasum >/dev/null; then
    checksum=(shasum -a 256)
else
    fail 'sha256sum or shasum is required'
fi

[[ -d "$output_dir" && ! -L "$output_dir" ]] || \
    fail 'output-dir must be an existing real directory'
destination="$output_dir/syft"
[[ ! -e "$destination" && ! -L "$destination" ]] || \
    fail "refusing to replace an existing Syft path: $destination"

version=1.51.1
case "$(uname -s):$(uname -m)" in
    Linux:x86_64)
        asset="syft_${version}_linux_amd64.tar.gz"
        expected=8fcb33017a0dc1058298c923c436d19dfa68ae93968e0b423248542e3afb9fc3
        ;;
    Linux:aarch64|Linux:arm64)
        asset="syft_${version}_linux_arm64.tar.gz"
        expected=a7fd2b784e6664acd44719270574f6cd8c6864fc2b1700bf9099bd1cccda7d7f
        ;;
    Darwin:x86_64)
        asset="syft_${version}_darwin_amd64.tar.gz"
        expected=0e186ce1d4351ec276126851ca3ff258ed070e93e73574ed64858d4fc2339867
        ;;
    Darwin:arm64)
        asset="syft_${version}_darwin_arm64.tar.gz"
        expected=ac063af3b9874769deb7ea1e6d76841e68f9e3bb50cd654226fc977de65532c1
        ;;
    *) fail "Syft is not qualified for host $(uname -s)/$(uname -m)" ;;
esac

archive=$(mktemp "${TMPDIR:-/tmp}/aros-syft.XXXXXX.tar.gz")
cleanup() {
    rm -f -- "$archive"
}
trap cleanup EXIT
url="https://github.com/anchore/syft/releases/download/v${version}/${asset}"
curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' --tlsv1.2 \
    "$url" --output "$archive"
measured=$("${checksum[@]}" "$archive" | awk '{ print $1 }')
[[ "$measured" == "$expected" ]] || \
    fail "Syft asset digest mismatch for $asset"

members=$(tar -tzf "$archive")
[[ $(grep -cx 'syft' <<<"$members") == 1 ]] || \
    fail 'pinned Syft archive does not contain exactly one root syft executable'
tar -xzf "$archive" -C "$output_dir" syft
[[ -f "$destination" && ! -L "$destination" ]] || \
    fail 'Syft extraction did not produce a regular executable'
chmod 0755 "$destination"
reported=$($destination version -o json | python3 -c '
import json
import sys

value = json.load(sys.stdin)
print(value.get("version", "") if isinstance(value, dict) else "")
')
[[ "$reported" == "$version" ]] || \
    fail "installed Syft reports ${reported:-no version}; expected $version"
printf '%s\n' "$destination"
