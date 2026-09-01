#!/usr/bin/env bash

# Validate and dearmor the one canonical public key accepted by the APT
# publication contract.  The primary-key identity is measured from the armored
# input before any import or dearmor operation.  A clean, option-free GnuPG
# home then re-exports the key so non-canonical armor cannot become an
# alternative representation of the same trust anchor.

set -euo pipefail

fail() {
    printf '::error::AP7240 %s\n' "$*" >&2
    exit 1
}

usage() {
    printf '%s\n' \
        'usage: verify-apt-public-key.sh --key FILE --fingerprint HEX' \
        '       --keyring-output FILE'
}

key=
fingerprint=
keyring_output=
while (($#)); do
    case "$1" in
        --key) key=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --keyring-output) keyring_output=${2:-}; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail "unknown APT public-key argument: $1" ;;
    esac
done

for command in awk cmp gpg gpgconf grep head install mv tail; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done
[[ -f "$key" && ! -L "$key" ]] || fail 'armored key must be one regular file'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail 'a full 40-hex primary-key fingerprint is required'
[[ -n "$keyring_output" && ! -e "$keyring_output" && ! -L "$keyring_output" ]] || \
    fail 'keyring output must be one new path'
fingerprint=${fingerprint^^}

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-apt-key.XXXXXX")
chmod 0700 "$work"
script_root=$(cd "$(dirname "$0")" && pwd)
"$script_root/prepare-output-parent.sh" --path "$keyring_output" --mode 0755
temporary=$(mktemp "${keyring_output}.tmp.XXXXXX")
cleanup() {
    trap - EXIT
    gpgconf --homedir "$work" --kill gpg-agent >/dev/null 2>&1 || true
    rm -rf -- "$work"
    rm -f -- "$temporary"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

snapshot="$work/key.asc"
install -m 0400 "$key" "$snapshot"
[[ $(head -n 1 "$snapshot") == '-----BEGIN PGP PUBLIC KEY BLOCK-----' && \
   $(tail -n 1 "$snapshot") == '-----END PGP PUBLIC KEY BLOCK-----' && \
   $(grep -c '^-----BEGIN PGP PUBLIC KEY BLOCK-----$' "$snapshot") == 1 && \
   $(grep -c '^-----END PGP PUBLIC KEY BLOCK-----$' "$snapshot") == 1 ]] || \
    fail 'public key has a non-canonical armor envelope'

primary_fingerprint=$(gpg --no-options --batch --with-colons --show-keys \
    --fingerprint "$snapshot" | awk -F: '
        $1 == "pub" {
            public_keys += 1
            validity = $2
            next
        }
        $1 == "fpr" && public_keys == 1 && !measured { measured = toupper($10) }
        END {
            if (public_keys != 1 || length(measured) != 40 ||
                measured !~ /^[0-9A-F]+$/ || validity ~ /^[redi]$/) exit 1
            print measured
        }
    ') || fail 'public key must contain exactly one active primary key'
[[ "$primary_fingerprint" == "$fingerprint" ]] || \
    fail 'public key has the wrong primary fingerprint'

gpg --no-options --batch --homedir "$work" --import "$snapshot" >/dev/null 2>&1 || \
    fail 'public key cannot be imported into an isolated keyring'
gpg --no-options --batch --homedir "$work" --armor --no-emit-version \
    --no-comments --export "$fingerprint" > "$work/canonical.asc"
cmp -s "$snapshot" "$work/canonical.asc" || \
    fail 'public key is not the canonical armored export of its measured primary key'

gpg --no-options --batch --homedir "$work" --yes --dearmor \
    --output "$temporary" "$snapshot"
mv "$temporary" "$keyring_output"
trap - EXIT HUP INT TERM
gpgconf --homedir "$work" --kill gpg-agent >/dev/null 2>&1 || true
rm -rf -- "$work"
