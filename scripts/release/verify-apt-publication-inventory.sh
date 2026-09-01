#!/usr/bin/env bash

set -euo pipefail

fail() {
    local code=$1
    shift
    printf '::error::%s %s\n' "$code" "$*" >&2
    exit 1
}

usage() {
    printf '%s\n' \
        'usage: verify-apt-publication-inventory.sh --directory DIR' \
        '       --mode full|metadata --fingerprint HEX [--version X.Y.Z]' \
        '       [--require-unexpired] [--now-epoch EPOCH]'
}

directory=
mode=
version=
fingerprint=
require_unexpired=false
now_epoch=
while (($#)); do
    case "$1" in
        --directory) directory=${2:-}; shift 2 ;;
        --mode) mode=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --require-unexpired) require_unexpired=true; shift ;;
        --now-epoch) now_epoch=${2:-}; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail AP7224 "unknown APT inventory argument: $1" ;;
    esac
done

[[ -d "$directory" && ! -L "$directory" ]] || \
    fail AP7224 'APT publication inventory must be a real directory'
[[ "$mode" == full || "$mode" == metadata ]] || \
    fail AP7224 'APT publication inventory mode must be full or metadata'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail AP7224 'APT publication inventory requires a full signing fingerprint'
fingerprint=${fingerprint^^}
if [[ -n "$now_epoch" ]]; then
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 && \
       "$now_epoch" =~ ^[1-9][0-9]*$ ]] || \
        fail AP7224 'APT clock override is restricted to release-policy fixtures'
fi
if [[ "$mode" == full ]]; then
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        fail AP7224 'full APT publication inventory requires canonical SemVer'
elif [[ -n "$version" ]]; then
    fail AP7224 'metadata-only APT inventory does not accept a version'
fi

for command in awk cmp find gpg gpgv grep gzip head sort tail wc; do
    command -v "$command" >/dev/null || \
        fail AP7224 "required APT inventory command is missing: $command"
done
if command -v sha256sum >/dev/null; then
    checksum=(sha256sum)
elif command -v shasum >/dev/null; then
    checksum=(shasum -a 256)
else
    fail AP7224 'SHA-256 implementation is missing'
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-apt-inventory.XXXXXX")
cleanup() {
    trap - EXIT
    gpgconf --homedir "$work/gnupg" --kill gpg-agent >/dev/null 2>&1 || true
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

if find "$directory" -mindepth 1 \( -type l -o ! -type f ! -type d \) \
        -print -quit | grep -q .; then
    fail AP7224 'APT publication handoff contains a symbolic link or special file'
fi

expected="$work/expected"
actual="$work/actual"
printf '%s\n' \
    aros-tools-archive-keyring.asc \
    dists/stable/InRelease \
    dists/stable/Release \
    dists/stable/Release.gpg > "$expected"

if [[ "$mode" == full ]]; then
    for arch in amd64 arm64; do
        prefix="dists/stable/main/binary-${arch}"
        printf '%s\n' \
            "pool/main/a/aros-tools/aros-tools_${version}_${arch}.deb" \
            "${prefix}/Packages" \
            "${prefix}/Packages.gz" >> "$expected"
        for index in Packages Packages.gz; do
            path="$directory/${prefix}/${index}"
            [[ -f "$path" && ! -L "$path" ]] || \
                fail AP7224 "APT publication handoff lacks $arch/$index"
            digest=$("${checksum[@]}" "$path" | awk '{ print $1 }')
            [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || \
                fail AP7224 "APT publication handoff has invalid $arch/$index digest"
            printf '%s\n' "${prefix}/by-hash/SHA256/${digest}" >> "$expected"
        done
    done
fi

(cd "$directory" && find . -type f -print | sed 's#^\./##' | sort) > "$actual"
sort -o "$expected" "$expected"
if ! cmp -s "$expected" "$actual"; then
    printf '%s\n' '::error::AP7224 APT publication handoff inventory is not closed' >&2
    diff -u "$expected" "$actual" >&2 || true
    exit 1
fi

install -d -m 0700 "$work/gnupg"
[[ $(head -n 1 "$directory/dists/stable/InRelease") == \
      '-----BEGIN PGP SIGNED MESSAGE-----' && \
   $(tail -n 1 "$directory/dists/stable/InRelease") == \
      '-----END PGP SIGNATURE-----' && \
   $(grep -c '^-----BEGIN PGP SIGNATURE-----$' \
      "$directory/dists/stable/InRelease") == 1 && \
   $(grep -c '^-----END PGP SIGNATURE-----$' \
      "$directory/dists/stable/InRelease") == 1 ]] || \
    fail AP7224 'InRelease has a non-canonical clearsign envelope'
[[ $(head -n 1 "$directory/dists/stable/Release.gpg") == \
      '-----BEGIN PGP SIGNATURE-----' && \
   $(tail -n 1 "$directory/dists/stable/Release.gpg") == \
      '-----END PGP SIGNATURE-----' && \
   $(grep -c '^-----BEGIN PGP SIGNATURE-----$' \
      "$directory/dists/stable/Release.gpg") == 1 && \
   $(grep -c '^-----END PGP SIGNATURE-----$' \
      "$directory/dists/stable/Release.gpg") == 1 ]] || \
    fail AP7224 'Release.gpg has a non-canonical signature envelope'
key_verifier=$(cd "$(dirname "$0")" && pwd)/verify-apt-public-key.sh
status_verifier=$(cd "$(dirname "$0")" && pwd)/verify-gpgv-status.sh
[[ -x "$key_verifier" && ! -L "$key_verifier" ]] || \
    fail AP7224 'canonical APT public-key verifier is missing or unsafe'
[[ -x "$status_verifier" && ! -L "$status_verifier" ]] || \
    fail AP7224 'canonical gpgv status verifier is missing or unsafe'
"$key_verifier" --key "$directory/aros-tools-archive-keyring.asc" \
    --fingerprint "$fingerprint" --keyring-output "$work/keyring.gpg" || \
    fail AP7224 'APT handoff keyring is not the canonical trust anchor'
gpgv --status-fd 3 --keyring "$work/keyring.gpg" \
    "$directory/dists/stable/InRelease" 3> "$work/inrelease.status" 2>/dev/null || \
    fail AP7224 'InRelease signature verification failed'
"$status_verifier" --status-file "$work/inrelease.status" \
    --fingerprint "$fingerprint" || \
    fail AP7224 'InRelease does not have one active required signature'
gpg --batch --homedir "$work/gnupg" --no-default-keyring \
    --keyring "$work/keyring.gpg" --decrypt \
    --output "$work/Release" "$directory/dists/stable/InRelease" >/dev/null 2>&1
cmp "$directory/dists/stable/Release" "$work/Release" >/dev/null || \
    fail AP7224 'InRelease does not authenticate the handed-off Release bytes'
gpgv --status-fd 3 --keyring "$work/keyring.gpg" \
    "$directory/dists/stable/Release.gpg" \
    "$directory/dists/stable/Release" 3> "$work/release.status" 2>/dev/null || \
    fail AP7224 'Release.gpg signature verification failed'
"$status_verifier" --status-file "$work/release.status" \
    --fingerprint "$fingerprint" || \
    fail AP7224 'Release.gpg does not have one active required signature'

[[ $(grep -c '^Acquire-By-Hash: yes$' \
        "$directory/dists/stable/Release") == 1 ]] || \
    fail AP7224 'Release must enable Acquire-By-Hash exactly once'
python3 - "$directory/dists/stable/Release" "$require_unexpired" "$now_epoch" <<'PY' || \
    fail AP7224 'Release has an invalid, missing, or expired Valid-Until'
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path
import sys

lines = Path(sys.argv[1]).read_text().splitlines()
values = [line.removeprefix('Valid-Until: ').strip() for line in lines if line.startswith('Valid-Until: ')]
dates = [line.removeprefix('Date: ').strip() for line in lines if line.startswith('Date: ')]
if len(values) != 1 or len(dates) != 1:
    raise SystemExit(1)
valid_until = parsedate_to_datetime(values[0]).astimezone(timezone.utc)
date = parsedate_to_datetime(dates[0]).astimezone(timezone.utc)
if valid_until <= date:
    raise SystemExit(1)
now = datetime.fromtimestamp(int(sys.argv[3]), timezone.utc) if sys.argv[3] else datetime.now(timezone.utc)
if sys.argv[2] == 'true' and valid_until <= now:
    raise SystemExit(1)
PY

if [[ "$mode" == full ]]; then
    for arch in amd64 arm64; do
        prefix="dists/stable/main/binary-${arch}"
        gzip -dc "$directory/${prefix}/Packages.gz" > "$work/Packages-${arch}"
        cmp "$directory/${prefix}/Packages" "$work/Packages-${arch}" >/dev/null || \
            fail AP7224 "compressed and plain Packages differ for $arch"
        [[ $(grep -c '^Package: aros-tools$' "$work/Packages-${arch}") == 1 ]] || \
            fail AP7224 "Packages inventory is not singular for $arch"
        [[ $(awk '/^Version: / { print $2; exit }' "$work/Packages-${arch}") == "${version}-1" ]] || \
            fail AP7224 "Packages version is wrong for $arch"
        [[ $(awk '/^Architecture: / { print $2; exit }' "$work/Packages-${arch}") == "$arch" ]] || \
            fail AP7224 "Packages architecture is wrong for $arch"
        package="pool/main/a/aros-tools/aros-tools_${version}_${arch}.deb"
        package_path="$directory/$package"
        package_digest=$("${checksum[@]}" "$package_path" | awk '{ print $1 }')
        package_size=$(wc -c < "$package_path" | tr -d ' ')
        [[ $(awk '/^Filename: / { print $2; exit }' "$work/Packages-${arch}") == "$package" && \
           $(awk '/^SHA256: / { print $2; exit }' "$work/Packages-${arch}") == "$package_digest" && \
           $(awk '/^Size: / { print $2; exit }' "$work/Packages-${arch}") == "$package_size" ]] || \
            fail AP7224 "Packages does not bind the handed-off Debian package for $arch"
        for index in Packages Packages.gz; do
            path="$directory/${prefix}/${index}"
            digest=$("${checksum[@]}" "$path" | awk '{ print $1 }')
            size=$(wc -c < "$path" | tr -d ' ')
            by_hash="$directory/${prefix}/by-hash/SHA256/${digest}"
            cmp "$path" "$by_hash" >/dev/null || \
                fail AP7224 "by-hash bytes differ for $arch/$index"
            read -r release_digest release_size < <(
                awk -v wanted="main/binary-${arch}/${index}" '
                    $1 == "SHA256:" { section = 1; next }
                    section && $0 !~ /^ / { section = 0 }
                    section && $3 == wanted { count += 1; digest = $1; size = $2 }
                    END { if (count != 1) exit 1; print digest, size }
                ' "$directory/dists/stable/Release"
            ) || true
            [[ "$release_digest" == "$digest" && "$release_size" == "$size" ]] || \
                fail AP7224 "Release digest or size differs for $arch/$index"
        done
    done
fi

printf 'verified closed %s APT publication inventory in %s\n' "$mode" "$directory"
