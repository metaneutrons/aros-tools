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
        'usage: build-apt-repository.sh --candidate-dir DIR --output-dir DIR' \
        '       --version VERSION --source-date-epoch EPOCH' \
        '       --private-key FILE --passphrase-file FILE --fingerprint HEX'
}

candidate_dir=
output_dir=
version=
source_date_epoch=
private_key=
passphrase_file=
fingerprint=

while (($#)); do
    case "$1" in
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        --output-dir) output_dir=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --source-date-epoch) source_date_epoch=${2:-}; shift 2 ;;
        --private-key) private_key=${2:-}; shift 2 ;;
        --passphrase-file) passphrase_file=${2:-}; shift 2 ;;
        --fingerprint) fingerprint=${2:-}; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail AP7200 "unknown APT builder argument: $1" ;;
    esac
done

for command in gpg gpgv; do
    command -v "$command" >/dev/null || fail AP7200 "required command is missing: $command"
done
if [[ "${AROS_APT_RENDER_LOCAL_FOR_TESTS:-}" == 1 ]]; then
    command -v python3 >/dev/null || fail AP7200 'python3 is required by the local policy fixture'
fi

[[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || \
    fail AP7200 'candidate-dir must be a real directory'
[[ -n "$output_dir" && ! -e "$output_dir" && ! -L "$output_dir" ]] || \
    fail AP7200 'output-dir must be a new path'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
    fail AP7200 'version must be a stable canonical SemVer'
[[ "$source_date_epoch" =~ ^[1-9][0-9]*$ ]] || \
    fail AP7200 'source-date-epoch must be a positive integer'
[[ -f "$private_key" && ! -L "$private_key" ]] || \
    fail AP7200 'private-key must be a regular file'
[[ -f "$passphrase_file" && ! -L "$passphrase_file" ]] || \
    fail AP7200 'passphrase-file must be a regular file'
[[ "$fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || \
    fail AP7200 'fingerprint must be a full 40-hex primary-key fingerprint'

script_root=$(cd "$(dirname "$0")" && pwd)
"$script_root/prepare-output-parent.sh" --path "$output_dir" --mode 0755
stage=$(mktemp -d "${output_dir}.tmp.XXXXXX")
cleanup() {
    rm -rf -- "$stage"
}
trap cleanup EXIT

renderer="$script_root/run-apt-metadata-renderer.sh"
[[ -x "$renderer" && ! -L "$renderer" ]] || fail AP7200 'APT metadata renderer is missing or unsafe'
"$renderer" "$candidate_dir" "$stage" "$version" "$source_date_epoch"

# Keep gpg-agent's Unix-domain socket below macOS' short sockaddr_un limit.
# APT publication itself runs on Linux, but this also keeps the hermetic policy
# fixture portable across supported developer hosts.
gnupg_home=$(mktemp -d /tmp/aros-apt-gpg.XXXXXX)
chmod 0700 "$gnupg_home"
cleanup_gnupg() {
    gpgconf --homedir "$gnupg_home" --kill gpg-agent >/dev/null 2>&1 || true
    rm -rf -- "$gnupg_home"
}
cleanup_all() {
    trap - EXIT
    cleanup_gnupg
    cleanup
}
trap cleanup_all EXIT
trap 'exit 130' HUP INT TERM
gpg --batch --homedir "$gnupg_home" --import "$private_key" >/dev/null
fingerprint=${fingerprint^^}
signing_key_verifier="$script_root/verify-apt-signing-key.sh"
[[ -x "$signing_key_verifier" && ! -L "$signing_key_verifier" ]] || \
    fail AP7203 'canonical APT signing-key verifier is missing or unsafe'
"$signing_key_verifier" --homedir "$gnupg_home" --fingerprint "$fingerprint" || \
    fail AP7203 'imported APT signing key is not the one active required key'
gpg --no-options --batch --homedir "$gnupg_home" --armor --no-emit-version \
    --no-comments --export "$fingerprint" \
    > "$stage/aros-tools-archive-keyring.asc"
gpg --batch --homedir "$gnupg_home" --yes --pinentry-mode loopback \
    --passphrase-file "$passphrase_file" --faked-system-time "${source_date_epoch}!" \
    --local-user "$fingerprint" --armor --detach-sign \
    --output "$stage/dists/stable/Release.gpg" "$stage/dists/stable/Release"
gpg --batch --homedir "$gnupg_home" --yes --pinentry-mode loopback \
    --passphrase-file "$passphrase_file" --faked-system-time "${source_date_epoch}!" \
    --local-user "$fingerprint" --armor --clearsign \
    --output "$stage/dists/stable/InRelease" "$stage/dists/stable/Release"
inventory=$(cd "$(dirname "$0")" && pwd)/verify-apt-publication-inventory.sh
[[ -x "$inventory" && ! -L "$inventory" ]] || \
    fail AP7203 'APT publication inventory verifier is missing or unsafe'
"$inventory" --directory "$stage" --mode full --version "$version" \
    --fingerprint "$fingerprint" >/dev/null

mv "$stage" "$output_dir"
trap - EXIT HUP INT TERM
cleanup_gnupg
