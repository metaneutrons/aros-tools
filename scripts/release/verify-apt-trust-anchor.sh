#!/usr/bin/env bash

# Bind workflow configuration and copy/paste documentation to the checked-in
# APT trust-anchor identity. Rotation therefore requires one explicit source
# change instead of a mutable repository variable alone.

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)

fail() {
    printf '::error::AP7244 %s\n' "$*" >&2
    exit 1
}

fingerprint=
documentation=
while (($#)); do
    case "$1" in
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --documentation) documentation=${2:-}; shift 2 ;;
        *) fail "unknown APT trust-anchor argument: $1" ;;
    esac
done

anchor="$root/contracts/apt-trust-anchor.txt"
[[ -f "$anchor" && ! -L "$anchor" ]] || fail 'checked-in APT trust anchor is missing or unsafe'
[[ $(wc -l < "$anchor" | tr -d ' ') == 1 ]] || fail 'APT trust anchor must contain exactly one line'
expected=$(tr -d '\r\n' < "$anchor")
[[ "$expected" =~ ^[0-9A-F]{40}$ ]] || fail 'checked-in APT trust anchor is malformed'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ && "${fingerprint^^}" == "$expected" ]] || \
    fail 'configured APT fingerprint differs from the checked-in trust anchor'

if [[ -n "$documentation" ]]; then
    [[ -f "$documentation" && ! -L "$documentation" ]] || \
        fail 'APT installation documentation is missing or unsafe'
    documented=$(awk -F= '
        $1 == "EXPECTED_FINGERPRINT" && $2 ~ /^[0-9A-F]{40}$/ {
            count += 1
            value = $2
        }
        END { if (count != 1) exit 1; print value }
    ' "$documentation") || \
        fail 'APT installation documentation must pin exactly one primary fingerprint'
    [[ "$documented" == "$expected" ]] || \
        fail 'APT installation documentation differs from the checked-in trust anchor'
fi
