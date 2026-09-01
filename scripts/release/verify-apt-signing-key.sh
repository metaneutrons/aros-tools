#!/usr/bin/env bash

# Validate the one active primary secret key imported for APT publication.

set -euo pipefail

fail() {
    printf '::error::AP7243 %s\n' "$*" >&2
    exit 1
}

homedir=
fingerprint=
signing_subkey=
while (($#)); do
    case "$1" in
        --homedir) homedir=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --signing-subkey) signing_subkey=${2:-}; shift 2 ;;
        *) fail "unknown APT signing-key argument: $1" ;;
    esac
done

[[ -d "$homedir" && ! -L "$homedir" ]] || fail 'GnuPG home must be a real directory'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'APT signing key requires a full 40-hex primary fingerprint'
fingerprint=${fingerprint^^}
if [[ -n "$signing_subkey" ]]; then
    [[ "$signing_subkey" =~ ^[0-9A-Fa-f]{40}$ ]] || \
        fail 'APT signing key requires a full 40-hex signing subkey fingerprint'
    signing_subkey=${signing_subkey^^}
fi

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

# When the caller names a signing subkey, the bundle must be a subkeys-only
# export: field 15 of the sec record is '#' when the primary secret is absent
# and '+' when it is present. That single character is the only machine-checkable
# evidence that the certify-only primary stayed offline, so it is enforced here
# rather than merely documented. The named subkey must be present in turn.
if [[ -n "$signing_subkey" ]]; then
    gpg --no-options --batch --homedir "$homedir" --with-colons \
        --list-secret-keys --fingerprint | awk -F: -v want="$signing_subkey" '
            $1 == "sec" { primary_stub = ($15 == "#"); next }
            $1 == "ssb" { current = $15; next }
            $1 == "fpr" && current != "" {
                if (toupper($10) == want && current == "+") subkey_present = 1
                current = ""
                next
            }
            END {
                if (!primary_stub) {
                    print "::error::AP7243 primary secret key is present; export secret subkeys only" > "/dev/stderr"
                    exit 1
                }
                if (!subkey_present) {
                    print "::error::AP7243 the named signing subkey is not present in the bundle" > "/dev/stderr"
                    exit 1
                }
            }
        ' || fail 'signing bundle does not match the required subkey-only shape'
fi
