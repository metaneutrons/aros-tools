#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7060 %s\n' "$*" >&2
    exit 1
}

artifact=
output=
kind=
version=
source_date_epoch=
syft=
while (($#)); do
    case "$1" in
        --artifact) artifact=${2:-}; shift 2 ;;
        --output) output=${2:-}; shift 2 ;;
        --kind) kind=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        --source-date-epoch) source_date_epoch=${2:-}; shift 2 ;;
        --syft) syft=${2:-}; shift 2 ;;
        *) fail "unknown SBOM generator argument: $1" ;;
    esac
done

command -v python3 >/dev/null || fail 'python3 is required'
if command -v sha256sum >/dev/null; then
    checksum=(sha256sum)
elif command -v shasum >/dev/null; then
    checksum=(shasum -a 256)
else
    fail 'sha256sum or shasum is required'
fi
[[ -f "$artifact" && ! -L "$artifact" ]] || fail 'artifact must be a regular file'
[[ ! -e "$output" && ! -L "$output" ]] || fail 'output must be a new path'
[[ "$kind" == archive || "$kind" == deb ]] || fail 'kind must be archive or deb'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || \
    fail 'version is not canonical release SemVer'
[[ "$source_date_epoch" =~ ^[1-9][0-9]*$ ]] || fail 'source-date-epoch must be positive'
[[ -x "$syft" && ! -L "$syft" ]] || fail 'syft must be a regular executable'
if [[ "$kind" == deb ]]; then
    command -v dpkg-deb >/dev/null || fail 'dpkg-deb is required for Debian SBOMs'
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-sbom.XXXXXX")
cleanup() {
    rm -rf -- "$work"
}
trap cleanup EXIT
payload="$work/payload"
mkdir "$payload"
if [[ "$kind" == archive ]]; then
    python3 - "$artifact" "$payload" <<'PY'
import pathlib
import sys
import tarfile

archive = pathlib.Path(sys.argv[1])
destination = pathlib.Path(sys.argv[2])
with tarfile.open(archive, 'r:gz') as source:
    members = source.getmembers()
    names = set()
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or not (member.isdir() or member.isfile()):
            raise SystemExit(f'unsafe archive member in SBOM subject: {member.name}')
        if member.name in names:
            raise SystemExit(f'duplicate archive member in SBOM subject: {member.name}')
        names.add(member.name)
    source.extractall(destination, members=members)
PY
    [[ $(find "$payload" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ') == 1 ]] || \
        fail 'archive must contain exactly one payload root'
    [[ -z $(find "$payload" -mindepth 1 -maxdepth 1 ! -type d -print -quit) ]] || \
        fail 'archive must not contain files outside its payload root'
    scan_root=$(find "$payload" -mindepth 1 -maxdepth 1 -type d -print -quit)
    binary_prefix=bin
else
    dpkg-deb --extract "$artifact" "$payload"
    scan_root=$payload
    binary_prefix=usr/bin
fi

expected=(
    aros aros-ahi-runner aros-collect aros-fetch
    aros-genmodule aros-romtool aros-transpiler aros-verify
)
[[ -d "$scan_root/$binary_prefix" && ! -L "$scan_root/$binary_prefix" ]] || \
    fail "artifact lacks $binary_prefix"
[[ $(find "$scan_root/$binary_prefix" -mindepth 1 -maxdepth 1 -type f | wc -l | tr -d ' ') == 8 ]] || \
    fail 'artifact must contain exactly eight regular binaries'
[[ -z $(find "$scan_root/$binary_prefix" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'artifact binary directory contains a non-regular entry'
for name in "${expected[@]}"; do
    [[ -f "$scan_root/$binary_prefix/$name" && \
       -x "$scan_root/$binary_prefix/$name" && \
       ! -L "$scan_root/$binary_prefix/$name" ]] || \
        fail "artifact binary is missing or unsafe: $name"
done

raw="$work/raw.json"
artifact_name=$(basename "$artifact")
SYFT_CHECK_FOR_APP_UPDATE=false \
SYFT_FILE_METADATA_SELECTION=all \
SYFT_FILE_METADATA_DIGESTS=sha256 \
    "$syft" scan "dir:$scan_root" \
        --source-name "$artifact_name" \
        --source-version "$version" \
        --output "spdx-json=$raw"

artifact_digest=$("${checksum[@]}" "$artifact" | awk '{ print $1 }')
created=$(python3 - "$source_date_epoch" <<'PY'
import datetime
import sys

print(datetime.datetime.fromtimestamp(
    int(sys.argv[1]), datetime.timezone.utc
).strftime('%Y-%m-%dT%H:%M:%SZ'))
PY
)
namespace="https://aros.metaneutrons.cc/spdx/aros-tools/${artifact_digest}"
python3 - "$raw" "$output" "$artifact_name" "$artifact_digest" \
    "$namespace" "$created" "$version" <<'PY'
import json
import pathlib
import sys

raw, output, artifact, digest, namespace, created, version = sys.argv[1:]
try:
    document = json.loads(pathlib.Path(raw).read_text(encoding='utf-8'))
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f'Syft output is not valid UTF-8 JSON: {error}')
if not isinstance(document, dict):
    raise SystemExit('Syft output is not a JSON object')
packages = document.get('packages')
if not isinstance(packages, list):
    raise SystemExit('Syft output has no package inventory')
roots = [item for item in packages if isinstance(item, dict) and item.get('name') == artifact]
if len(roots) != 1:
    raise SystemExit('Syft output does not have exactly one artifact root package')
creation = document.get('creationInfo')
if not isinstance(creation, dict):
    raise SystemExit('Syft output has no creationInfo object')
document['name'] = artifact
document['documentNamespace'] = namespace
creation['created'] = created
root = roots[0]
root['versionInfo'] = version
root['packageFileName'] = artifact
root['checksums'] = [{'algorithm': 'SHA256', 'checksumValue': digest}]
pathlib.Path(output).write_text(
    json.dumps(document, ensure_ascii=False, sort_keys=True, separators=(',', ':')) + '\n',
    encoding='utf-8',
)
PY

[[ -s "$output" ]] || fail 'SBOM generator produced an empty document'
