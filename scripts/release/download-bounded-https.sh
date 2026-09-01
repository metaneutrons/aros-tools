#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7045 %s\n' "$*" >&2
    exit 1
}

url=
source_file=
output=
class=
expected_bytes=
allow_not_found=false
while (($#)); do
    case "$1" in
        --url) url=${2:-}; shift 2 ;;
        --source-file) source_file=${2:-}; shift 2 ;;
        --output) output=${2:-}; shift 2 ;;
        --class) class=${2:-}; shift 2 ;;
        --expected-bytes) expected_bytes=${2:-}; shift 2 ;;
        --allow-not-found) allow_not_found=true; shift ;;
        *) fail "unknown bounded-download argument: $1" ;;
    esac
done

case "$class" in
    apt-key|apt-signature) maximum=$((1024 * 1024)) ;;
    apt-release) maximum=$((2 * 1024 * 1024)) ;;
    apt-index) maximum=$((64 * 1024 * 1024)) ;;
    apt-package) maximum=$((512 * 1024 * 1024)) ;;
    json) maximum=$((4 * 1024 * 1024)) ;;
    *) fail 'download class is missing or unknown' ;;
esac
[[ -n "$output" && ! -e "$output" && ! -L "$output" ]] || \
    fail 'download output must be one new path'
if [[ -n "$url" && -n "$source_file" ]] || [[ -z "$url" && -z "$source_file" ]]; then
    fail 'select exactly one HTTPS URL or fixture source file'
fi
if [[ -n "$expected_bytes" ]]; then
    [[ "$expected_bytes" =~ ^[1-9][0-9]*$ && "$expected_bytes" -le "$maximum" ]] || \
        fail "expected size is outside the $class ceiling"
fi

script_root=$(cd "$(dirname "$0")" && pwd)
"$script_root/prepare-output-parent.sh" --path "$output" --mode 0755
temporary=$(mktemp "${output}.tmp.XXXXXX")
cleanup() {
    trap - EXIT
    rm -f -- "$temporary"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

if [[ -n "$source_file" ]]; then
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'source-file is restricted to release-policy fixtures'
    if [[ ! -e "$source_file" && ! -L "$source_file" && "$allow_not_found" == true ]]; then
        cleanup
        trap - EXIT HUP INT TERM
        exit 44
    fi
    [[ -f "$source_file" && ! -L "$source_file" ]] || \
        fail 'fixture source must be one regular file'
    measured=$(wc -c < "$source_file" | tr -d ' ')
    [[ "$measured" -le "$maximum" ]] || fail "fixture exceeds the $class ceiling"
    if [[ -n "$expected_bytes" && "$measured" != "$expected_bytes" ]]; then
        fail 'fixture size differs from the signed expected size'
    fi
    install -m 0644 "$source_file" "$temporary"
else
    [[ "$url" =~ ^https:// ]] || fail 'download URL must use HTTPS'
    status=$(curl --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --retry 4 --retry-all-errors --retry-delay 2 \
        --max-filesize "${expected_bytes:-$maximum}" \
        --write-out '%{http_code}' --output "$temporary" "$url") || \
        fail "bounded HTTPS request failed for class $class"
    case "$status" in
        200) ;;
        404)
            if [[ "$allow_not_found" == true ]]; then
                cleanup
                trap - EXIT HUP INT TERM
                exit 44
            fi
            fail 'required HTTPS object does not exist'
            ;;
        *) fail "HTTPS request returned status $status" ;;
    esac
    measured=$(wc -c < "$temporary" | tr -d ' ')
    [[ "$measured" -le "$maximum" ]] || fail "response exceeds the $class ceiling"
    if [[ -n "$expected_bytes" && "$measured" != "$expected_bytes" ]]; then
        fail 'response size differs from the signed expected size'
    fi
fi

mv "$temporary" "$output"
trap - EXIT HUP INT TERM
