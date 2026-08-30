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

for command in apt-ftparchive date dpkg-deb dpkg-scanpackages gpg gpgv gzip; do
    command -v "$command" >/dev/null || fail AP7200 "required command is missing: $command"
done

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

output_parent=$(dirname "$output_dir")
install -d "$output_parent"
stage=$(mktemp -d "${output_dir}.tmp.XXXXXX")
cleanup() {
    rm -rf -- "$stage"
}
trap cleanup EXIT

pool="$stage/pool/main/a/aros-tools"
install -d "$pool"
for arch in amd64 arm64; do
    deb="$candidate_dir/aros-tools_${version}_${arch}.deb"
    [[ -f "$deb" && ! -L "$deb" ]] || \
        fail AP7201 "qualified Debian package is missing: $deb"
    package_version=$(dpkg-deb --field "$deb" Version)
    package_arch=$(dpkg-deb --field "$deb" Architecture)
    if [[ "$package_version" != "${version}-1" || "$package_arch" != "$arch" ]]; then
        fail AP7202 "Debian identity mismatch for $deb"
    fi
    install -m 0644 "$deb" "$pool/"
done

for arch in amd64 arm64; do
    binary_dir="$stage/dists/stable/main/binary-${arch}"
    install -d "$binary_dir"
    (
        cd "$stage"
        dpkg-scanpackages --arch "$arch" pool/main/a/aros-tools /dev/null
    ) > "$binary_dir/Packages"
    gzip -n -9 -c "$binary_dir/Packages" > "$binary_dir/Packages.gz"
done

release_date=$(date --utc --date="@${source_date_epoch}" --rfc-email)
(
    cd "$stage"
    apt-ftparchive \
        -o APT::FTPArchive::Release::Origin='AROS tools' \
        -o APT::FTPArchive::Release::Label='AROS tools' \
        -o APT::FTPArchive::Release::Suite=stable \
        -o APT::FTPArchive::Release::Codename=stable \
        -o APT::FTPArchive::Release::Architectures='amd64 arm64' \
        -o APT::FTPArchive::Release::Components=main \
        -o APT::FTPArchive::Release::Description='Signed aros-tools packages' \
        -o APT::FTPArchive::Release::Date="$release_date" \
        release dists/stable
) > "$stage/dists/stable/Release"

gnupg_home=$(mktemp -d "${output_dir}.gnupg.XXXXXX")
chmod 0700 "$gnupg_home"
cleanup_gnupg() {
    rm -rf -- "$gnupg_home"
}
trap 'cleanup_gnupg; cleanup' EXIT
gpg --batch --homedir "$gnupg_home" --import "$private_key" >/dev/null 2>&1
fingerprint=${fingerprint^^}
measured=$(gpg --batch --homedir "$gnupg_home" --with-colons \
    --list-secret-keys --fingerprint "$fingerprint" | \
    awk -F: '$1 == "fpr" { print toupper($10); exit }')
[[ "$measured" == "$fingerprint" ]] || \
    fail AP7203 'imported APT signing key has the wrong fingerprint'
gpg --batch --homedir "$gnupg_home" --armor --export "$fingerprint" \
    > "$stage/aros-tools-archive-keyring.asc"
gpg --batch --homedir "$gnupg_home" --yes --pinentry-mode loopback \
    --passphrase-file "$passphrase_file" --faked-system-time "${source_date_epoch}!" \
    --local-user "$fingerprint" --armor --detach-sign \
    --output "$stage/dists/stable/Release.gpg" "$stage/dists/stable/Release"
gpg --batch --homedir "$gnupg_home" --yes --pinentry-mode loopback \
    --passphrase-file "$passphrase_file" --faked-system-time "${source_date_epoch}!" \
    --local-user "$fingerprint" --armor --clearsign \
    --output "$stage/dists/stable/InRelease" "$stage/dists/stable/Release"
gpg --batch --dearmor < "$stage/aros-tools-archive-keyring.asc" \
    > "$stage/archive-keyring.gpg"
gpgv --keyring "$stage/archive-keyring.gpg" "$stage/dists/stable/InRelease"
gpgv --keyring "$stage/archive-keyring.gpg" \
    "$stage/dists/stable/Release.gpg" "$stage/dists/stable/Release"
rm "$stage/archive-keyring.gpg"

mv "$stage" "$output_dir"
trap - EXIT
cleanup_gnupg
