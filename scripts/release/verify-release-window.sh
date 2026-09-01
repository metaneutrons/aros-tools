#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7070 %s\n' "$*" >&2
    exit 1
}

tag_date_epoch=
max_age_seconds=604800
while (($#)); do
    case "$1" in
        --tag-date-epoch) tag_date_epoch=${2:-}; shift 2 ;;
        --max-age-seconds) max_age_seconds=${2:-}; shift 2 ;;
        *) fail "unknown release-window verifier argument: $1" ;;
    esac
done

[[ "$tag_date_epoch" =~ ^[1-9][0-9]*$ ]] || fail 'tag-date-epoch must be positive'
[[ "$max_age_seconds" =~ ^[1-9][0-9]*$ ]] || fail 'max-age-seconds must be positive'
now=${AROS_RELEASE_NOW_EPOCH:-$(date +%s)}
[[ "$now" =~ ^[1-9][0-9]*$ ]] || fail 'current time is malformed'
((tag_date_epoch <= now + 300)) || fail 'annotated tag timestamp is more than five minutes in the future'
((now - tag_date_epoch <= max_age_seconds)) || \
    fail "annotated tag is older than the ${max_age_seconds}-second mutation window"
printf '%s\n' "$now"
