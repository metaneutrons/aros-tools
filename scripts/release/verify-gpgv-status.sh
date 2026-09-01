#!/usr/bin/env bash

# Validate one gpgv --status-fd transcript. gpgv may exit zero while reporting
# an expired or revoked key alongside VALIDSIG, so process status alone is not
# a trust decision.

set -euo pipefail

fail() {
    printf '::error::AP7242 %s\n' "$*" >&2
    exit 1
}

status_file=
fingerprint=
while (($#)); do
    case "$1" in
        --status-file) status_file=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        *) fail "unknown gpgv-status argument: $1" ;;
    esac
done

[[ -f "$status_file" && ! -L "$status_file" ]] || \
    fail 'gpgv status must be one regular file'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'gpgv status requires a full 40-hex primary fingerprint'
fingerprint=${fingerprint^^}

awk -v expected="$fingerprint" '
    $1 != "[GNUPG:]" { next }
    $2 == "NEWSIG" { newsig += 1; next }
    $2 == "GOODSIG" { goodsig += 1; next }
    $2 == "VALIDSIG" {
        validsig += 1
        primary = toupper($NF)
        next
    }
    $2 ~ /^(REVKEYSIG|KEYREVOKED|EXPKEYSIG|EXPSIG|KEYEXPIRED|SIGEXPIRED|BADSIG|ERRSIG|NO_PUBKEY|FAILURE|NODATA|UNEXPECTED|BADARMOR|ERROR|DECRYPTION_FAILED)$/ {
        invalid += 1
        invalid_status = $2
    }
    END {
        if (invalid != 0) {
            print "::error::AP7242 gpgv reported invalid status " invalid_status > "/dev/stderr"
            exit 1
        }
        if (newsig != 1 || goodsig != 1 || validsig != 1) {
            print "::error::AP7242 gpgv transcript must contain exactly one NEWSIG, GOODSIG and VALIDSIG" > "/dev/stderr"
            exit 1
        }
        if (length(primary) != 40 || primary !~ /^[0-9A-F]+$/ || primary != expected) {
            print "::error::AP7242 gpgv transcript has the wrong primary signer" > "/dev/stderr"
            exit 1
        }
    }
' "$status_file"
