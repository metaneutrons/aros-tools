#!/usr/bin/env bash

# Validate the immutable base of the current stable APT channel before a
# same-version metadata repair. Mutable aliases and the detached Release files
# may be absent or damaged; a present InRelease remains the authoritative
# commit point and must authenticate exactly the expected index identities.

set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)

fail() {
    printf '::error::AP7237 %s\n' "$*" >&2
    exit 1
}

usage() {
    printf '%s\n' \
        'usage: verify-apt-recovery-base.sh --expected-directory DIR' \
        '       --fingerprint HEX --version X.Y.Z' \
        '       (--base-url HTTPS_URL | --source-directory DIR)'
}

expected_directory=
fingerprint=
version=
base_url=
source_directory=
while (($#)); do
    case "$1" in
        --expected-directory) expected_directory=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --base-url) base_url=${2:-}; shift 2 ;;
        --source-directory) source_directory=${2:-}; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail "unknown APT recovery argument: $1" ;;
    esac
done

[[ -d "$expected_directory" && ! -L "$expected_directory" ]] || \
    fail 'expected APT repository must be a real directory'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'a full 40-hex primary-key fingerprint is required'
fingerprint=${fingerprint^^}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    fail 'version must be canonical SemVer'
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
expected_directory=$(cd "$expected_directory" && pwd -P)

for command in awk cmp curl find gpg gpgv grep gzip head install python3 sort tail wc; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done
if command -v sha256sum >/dev/null; then
    checksum=(sha256sum)
elif command -v shasum >/dev/null; then
    checksum=(shasum -a 256)
else
    fail 'SHA-256 implementation is missing'
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-apt-recovery.XXXXXX")
cleanup() {
    trap - EXIT
    gpgconf --homedir "$work/gnupg" --kill gpg-agent >/dev/null 2>&1 || true
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

if find "$expected_directory" -mindepth 1 \
        \( -type l -o ! -type f ! -type d \) -print -quit | grep -q .; then
    fail 'expected APT repository contains a symbolic link or special file'
fi

expected_inventory="$work/expected-inventory"
actual_inventory="$work/actual-inventory"
: > "$expected_inventory"
printf '%s\n' dists/stable/Release >> "$expected_inventory"
for arch in amd64 arm64; do
    prefix="dists/stable/main/binary-${arch}"
    package="pool/main/a/aros-tools/aros-tools_${version}_${arch}.deb"
    printf '%s\n' "$package" "$prefix/Packages" "$prefix/Packages.gz" \
        >> "$expected_inventory"
    for index in Packages Packages.gz; do
        path="$expected_directory/$prefix/$index"
        [[ -f "$path" && ! -L "$path" ]] || \
            fail "expected APT repository lacks $arch/$index"
        digest=$("${checksum[@]}" "$path" | awk '{ print $1 }')
        printf '%s\n' "$prefix/by-hash/SHA256/$digest" >> "$expected_inventory"
    done
done
(cd "$expected_directory" && find . -type f -print | sed 's#^\./##' | sort) \
    > "$actual_inventory"
sort -o "$expected_inventory" "$expected_inventory"
cmp "$expected_inventory" "$actual_inventory" >/dev/null || \
    fail 'expected unsigned APT repository inventory is not closed'

fetch_required() {
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
        fail "bounded APT recovery download failed for $relative"
}

fetch_optional() {
    local relative=$1 output=$2 class=$3 expected_bytes=${4:-} source status
    local arguments=(--output "$output" --class "$class" --allow-not-found)
    [[ -z "$expected_bytes" ]] || arguments+=(--expected-bytes "$expected_bytes")
    if [[ -n "$source_directory" ]]; then
        source="$source_directory/$relative"
        arguments+=(--source-file "$source")
    else
        arguments+=(--url "$base_url/$relative")
    fi
    set +e
    AROS_RELEASE_POLICY_FIXTURE="${AROS_RELEASE_POLICY_FIXTURE:-}" \
        "$root/scripts/release/download-bounded-https.sh" "${arguments[@]}"
    status=$?
    set -e
    case "$status" in
        0) return 0 ;;
        44) return 1 ;;
        *) fail "bounded optional APT recovery download failed for $relative" ;;
    esac
}

fetch_required aros-tools-archive-keyring.asc "$work/key.asc" apt-key
key_verifier="$root/scripts/release/verify-apt-public-key.sh"
status_verifier="$root/scripts/release/verify-gpgv-status.sh"
[[ -x "$key_verifier" && ! -L "$key_verifier" ]] || \
    fail 'canonical APT public-key verifier is missing or unsafe'
[[ -x "$status_verifier" && ! -L "$status_verifier" ]] || \
    fail 'canonical gpgv status verifier is missing or unsafe'
"$key_verifier" --key "$work/key.asc" --fingerprint "$fingerprint" \
    --keyring-output "$work/keyring.gpg" || \
    fail 'public APT keyring is not the canonical trust anchor'
install -d -m 0700 "$work/gnupg"

committed=false
if fetch_optional dists/stable/InRelease "$work/InRelease" apt-release; then
    [[ $(head -n 1 "$work/InRelease") == '-----BEGIN PGP SIGNED MESSAGE-----' && \
       $(tail -n 1 "$work/InRelease") == '-----END PGP SIGNATURE-----' && \
       $(grep -c '^-----BEGIN PGP SIGNATURE-----$' "$work/InRelease") == 1 && \
       $(grep -c '^-----END PGP SIGNATURE-----$' "$work/InRelease") == 1 ]] || \
        fail 'public InRelease has a non-canonical clearsign envelope'
    gpgv --status-fd 3 --keyring "$work/keyring.gpg" \
        "$work/InRelease" 3> "$work/inrelease.status" 2>/dev/null || \
        fail 'public InRelease signature verification failed'
    "$status_verifier" --status-file "$work/inrelease.status" \
        --fingerprint "$fingerprint" || \
        fail 'public InRelease does not have one active required signature'
    gpg --batch --homedir "$work/gnupg" --no-default-keyring \
        --keyring "$work/keyring.gpg" --decrypt --output "$work/committed-Release" \
        "$work/InRelease" >/dev/null 2>&1 || \
        fail 'public InRelease payload cannot be extracted'
    [[ $(grep -c '^Acquire-By-Hash: yes$' "$work/committed-Release") == 1 ]] || \
        fail 'committed Release must enable Acquire-By-Hash exactly once'
    python3 - "$work/committed-Release" <<'PY' || \
        fail 'committed Release has invalid or ambiguous dates'
from email.utils import parsedate_to_datetime
from pathlib import Path
import sys

lines = Path(sys.argv[1]).read_text().splitlines()
dates = [line.removeprefix('Date: ').strip() for line in lines if line.startswith('Date: ')]
valids = [line.removeprefix('Valid-Until: ').strip() for line in lines if line.startswith('Valid-Until: ')]
if len(dates) != 1 or len(valids) != 1 or parsedate_to_datetime(valids[0]) <= parsedate_to_datetime(dates[0]):
    raise SystemExit(1)
PY
    committed=true
fi

for arch in amd64 arm64; do
    prefix="dists/stable/main/binary-${arch}"
    plain="$expected_directory/$prefix/Packages"
    compressed="$expected_directory/$prefix/Packages.gz"
    gzip -dc "$compressed" > "$work/Packages-${arch}"
    cmp "$plain" "$work/Packages-${arch}" >/dev/null || \
        fail "expected compressed and plain Packages differ for $arch"
    [[ $(grep -c '^Package: aros-tools$' "$plain") == 1 && \
       $(awk '/^Version: / { print $2 }' "$plain") == "${version}-1" && \
       $(awk '/^Architecture: / { print $2 }' "$plain") == "$arch" ]] || \
        fail "expected Packages identity is malformed for $arch"
    package="pool/main/a/aros-tools/aros-tools_${version}_${arch}.deb"
    package_digest=$("${checksum[@]}" "$expected_directory/$package" | awk '{ print $1 }')
    package_size=$(wc -c < "$expected_directory/$package" | tr -d ' ')
    [[ $(awk '/^Filename: / { print $2 }' "$plain") == "$package" && \
       $(awk '/^SHA256: / { print $2 }' "$plain") == "$package_digest" && \
       $(awk '/^Size: / { print $2 }' "$plain") == "$package_size" ]] || \
        fail "expected Packages does not bind the package for $arch"
    fetch_required "$package" "$work/public-${arch}.deb" apt-package "$package_size"
    cmp "$expected_directory/$package" "$work/public-${arch}.deb" >/dev/null || \
        fail "public immutable Debian package differs for $arch"
    for index in Packages Packages.gz; do
        expected="$expected_directory/$prefix/$index"
        digest=$("${checksum[@]}" "$expected" | awk '{ print $1 }')
        size=$(wc -c < "$expected" | tr -d ' ')
        cmp "$expected" "$expected_directory/$prefix/by-hash/SHA256/$digest" \
            >/dev/null || fail "expected by-hash object differs for $arch/$index"
        if [[ "$committed" == true ]]; then
            read -r signed_digest signed_size < <(
                awk -v wanted="main/binary-${arch}/${index}" '
                    $1 == "SHA256:" { section = 1; next }
                    section && $0 !~ /^ / { section = 0 }
                    section && $3 == wanted { count += 1; digest = $1; size = $2 }
                    END { if (count != 1) exit 1; print digest, size }
                ' "$work/committed-Release"
            ) || fail "committed Release lacks singular $arch/$index identity"
            [[ "$signed_digest" == "$digest" && "$signed_size" == "$size" ]] || \
                fail "committed Release differs from expected $arch/$index"
        fi
        fetch_required "$prefix/by-hash/SHA256/$digest" \
            "$work/by-hash-${index}-${arch}" apt-index "$size"
        cmp "$expected" "$work/by-hash-${index}-${arch}" >/dev/null || \
            fail "public immutable by-hash object differs for $arch/$index"
        # Mutable aliases are deliberately optional here. If present, exact
        # bytes are recorded as converged; missing or divergent bytes are
        # repaired later under the R2 snapshot CAS.
        if fetch_optional "$prefix/$index" "$work/alias-${index}-${arch}" \
             apt-index && \
           ! cmp -s "$expected" "$work/alias-${index}-${arch}"; then
            printf 'recoverable divergent alias: %s/%s\n' "$arch" "$index" >&2
        fi
    done
done

if [[ "$committed" == true ]]; then
    printf '%s\n' committed
else
    printf '%s\n' missing-commit-point
fi
