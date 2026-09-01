#!/usr/bin/env bash

# Validate the one active primary secret key imported for APT publication.

set -euo pipefail

fail() {
    printf '::error::AP7243 %s\n' "$*" >&2
    exit 1
}

homedir=
fingerprint=
while (($#)); do
    case "$1" in
        --homedir) homedir=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        *) fail "unknown APT signing-key argument: $1" ;;
    esac
done

[[ -d "$homedir" && ! -L "$homedir" ]] || fail 'GnuPG home must be a real directory'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'APT signing key requires a full 40-hex primary fingerprint'
fingerprint=${fingerprint^^}

measured=$(gpg --no-options --batch --homedir "$homedir" --with-colons \
    --list-secret-keys --fingerprint | awk -F: '
        $1 == "sec" {
            secret_keys += 1
            validity = $2
            next
        }
        $1 == "fpr" && secret_keys == 1 && !measured { measured = toupper($10) }
        END {
            if (secret_keys != 1 || length(measured) != 40 ||
                measured !~ /^[0-9A-F]+$/ || validity ~ /^[redi]$/) exit 1
            print measured
        }
    ') || fail 'signing bundle must contain exactly one active primary secret key'
[[ "$measured" == "$fingerprint" ]] || fail 'signing key has the wrong primary fingerprint'
