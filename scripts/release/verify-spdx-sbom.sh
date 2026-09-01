#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7030 %s\n' "$*" >&2
    exit 1
}

artifact=
sbom=
expected_sha256=
kind=
version=
while (($#)); do
    case "$1" in
        --artifact) artifact=${2:-}; shift 2 ;;
        --sbom) sbom=${2:-}; shift 2 ;;
        --expected-sha256) expected_sha256=${2:-}; shift 2 ;;
        --kind) kind=${2:-}; shift 2 ;;
        --version) version=${2:-}; shift 2 ;;
        *) fail "unknown SBOM verifier argument: $1" ;;
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

[[ -f "$artifact" && ! -L "$artifact" ]] || \
    fail "artifact must be a regular file: $artifact"
[[ -f "$sbom" && ! -L "$sbom" ]] || \
    fail "SBOM must be a regular file: $sbom"
[[ "$expected_sha256" =~ ^[0-9a-f]{64}$ ]] || \
    fail 'expected artifact SHA-256 must be 64 lowercase hexadecimal characters'
[[ "$kind" == archive || "$kind" == deb ]] || fail 'kind must be archive or deb'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || \
    fail 'version is not canonical release SemVer'

measured_sha256=$("${checksum[@]}" "$artifact" | awk '{ print $1 }')
[[ "$measured_sha256" == "$expected_sha256" ]] || \
    fail "artifact digest does not match its qualified identity: $artifact"

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-sbom-verify.XXXXXX")
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
    command -v dpkg-deb >/dev/null || fail 'dpkg-deb is required for Debian verification'
    dpkg-deb --extract "$artifact" "$payload"
    scan_root=$payload
    binary_prefix=usr/bin
fi

expected_names=(
    aros aros-ahi-runner aros-collect aros-fetch
    aros-genmodule aros-romtool aros-transpiler aros-verify
)
binary_dir="$scan_root/$binary_prefix"
[[ -d "$binary_dir" && ! -L "$binary_dir" ]] || fail "artifact lacks $binary_prefix"
[[ $(find "$binary_dir" -mindepth 1 -maxdepth 1 -type f | wc -l | tr -d ' ') == 8 ]] || \
    fail 'artifact does not contain exactly eight regular binaries'
[[ -z $(find "$binary_dir" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'artifact binary directory contains a non-regular entry'
expected_file="$work/expected.tsv"
: > "$expected_file"
for name in "${expected_names[@]}"; do
    binary="$binary_dir/$name"
    [[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] || \
        fail "artifact binary is missing, non-executable or unsafe: $name"
    digest=$("${checksum[@]}" "$binary" | awk '{ print $1 }')
    printf '%s\t%s\n' "$binary_prefix/$name" "$digest" >> "$expected_file"
done

python3 - "$sbom" "$expected_file" "$(basename "$artifact")" \
    "$measured_sha256" "$version" <<'PY'
import datetime
import json
import pathlib
import sys

sbom_path, expected_path, artifact, digest, version = sys.argv[1:]
try:
    document = json.loads(pathlib.Path(sbom_path).read_text(encoding='utf-8'))
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f'SBOM is not valid UTF-8 JSON: {error}')

def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(message)

require(isinstance(document, dict), 'SPDX document must be a JSON object')
require(document.get('spdxVersion') == 'SPDX-2.3', 'SPDX version must be 2.3')
require(document.get('dataLicense') == 'CC0-1.0', 'SPDX data license must be CC0-1.0')
require(document.get('SPDXID') == 'SPDXRef-DOCUMENT', 'SPDX document ID is invalid')
require(document.get('name') == artifact, 'SPDX document name does not match the artifact')
require(
    document.get('documentNamespace') == f'https://aros.metaneutrons.cc/spdx/aros-tools/{digest}',
    'SPDX namespace does not bind the qualified artifact digest',
)
creation = document.get('creationInfo')
require(isinstance(creation, dict), 'SPDX creationInfo must be an object')
created = creation.get('created')
require(isinstance(created, str), 'SPDX creation timestamp is missing')
try:
    datetime.datetime.strptime(created, '%Y-%m-%dT%H:%M:%SZ')
except ValueError as error:
    raise SystemExit(f'SPDX creation timestamp is not canonical UTC: {error}')
creators = creation.get('creators')
require(
    isinstance(creators, list) and creators and all(isinstance(item, str) and item for item in creators),
    'SPDX creators must be a non-empty string array',
)
require(
    creators.count('Tool: syft-1.51.1') == 1,
    'SPDX creators do not identify exactly the pinned Syft 1.51.1 producer',
)

packages = document.get('packages')
files = document.get('files')
relationships = document.get('relationships')
require(isinstance(packages, list) and packages, 'SPDX packages must be a non-empty array')
require(isinstance(files, list), 'SPDX files must be an array')
require(isinstance(relationships, list), 'SPDX relationships must be an array')
for group, label in ((packages, 'package'), (files, 'file'), (relationships, 'relationship')):
    require(all(isinstance(item, dict) for item in group), f'every SPDX {label} must be an object')

roots = [package for package in packages if package.get('name') == artifact]
require(len(roots) == 1, 'SPDX must contain exactly one artifact root package')
root = roots[0]
root_id = root.get('SPDXID')
require(isinstance(root_id, str) and root_id.startswith('SPDXRef-'), 'artifact root SPDXID is invalid')
require(root.get('versionInfo') == version, 'artifact root version does not match release version')
require(root.get('packageFileName') == artifact, 'artifact root packageFileName is invalid')
require(
    isinstance(root.get('downloadLocation'), str) and root.get('downloadLocation'),
    'artifact root downloadLocation is missing',
)
require(isinstance(root.get('filesAnalyzed'), bool), 'artifact root filesAnalyzed is missing')
for field in ('licenseConcluded', 'licenseDeclared', 'copyrightText'):
    require(
        isinstance(root.get(field), str) and root.get(field),
        f'artifact root {field} is missing',
    )
root_checksums = root.get('checksums')
root_sha256 = [item for item in root_checksums or [] if isinstance(item, dict) and
               item.get('algorithm') == 'SHA256']
require(
    isinstance(root_checksums, list) and len(root_sha256) == 1 and
    root_sha256[0].get('checksumValue') == digest,
    'artifact root does not have exactly its qualified SHA-256 digest',
)
require(any(
    relation.get('spdxElementId') == 'SPDXRef-DOCUMENT' and
    relation.get('relationshipType') == 'DESCRIBES' and
    relation.get('relatedSpdxElement') == root_id
    for relation in relationships
), 'SPDX document does not DESCRIBE the artifact root')

all_ids = ['SPDXRef-DOCUMENT']
for element in packages + files:
    identifier = element.get('SPDXID')
    require(isinstance(identifier, str) and identifier.startswith('SPDXRef-'), 'SPDX element ID is invalid')
    all_ids.append(identifier)
require(len(all_ids) == len(set(all_ids)), 'SPDX element IDs are not unique')
known_ids = set(all_ids)
for relation in relationships:
    require(relation.get('spdxElementId') in known_ids, 'SPDX relationship has an unknown source')
    require(
        relation.get('relatedSpdxElement') in known_ids or
        relation.get('relatedSpdxElement') == 'NONE',
        'SPDX relationship has an unknown destination',
    )
    require(isinstance(relation.get('relationshipType'), str), 'SPDX relationship type is missing')

expected = {}
for line in pathlib.Path(expected_path).read_text(encoding='utf-8').splitlines():
    name, checksum = line.split('\t', 1)
    expected[name] = checksum
require(len(expected) == 8, 'internal binary expectation is not exactly eight files')
binary_prefix = next(iter(expected)).rsplit('/', 1)[0] + '/'
reported = {}
for entry in files:
    name = entry.get('fileName')
    if not isinstance(name, str) or not name.startswith(binary_prefix):
        continue
    require(name in expected, f'SPDX contains unexpected release binary: {name}')
    require(name not in reported, f'SPDX contains duplicate release binary: {name}')
    require(
        isinstance(entry.get('licenseConcluded'), str) and entry.get('licenseConcluded'),
        f'SPDX binary license is missing: {name}',
    )
    require(
        isinstance(entry.get('copyrightText'), str) and entry.get('copyrightText'),
        f'SPDX binary copyright is missing: {name}',
    )
    checksums = entry.get('checksums')
    require(isinstance(checksums, list), f'SPDX binary has no checksums: {name}')
    sha256 = [item for item in checksums if isinstance(item, dict) and
              item.get('algorithm') == 'SHA256']
    require(
        len(sha256) == 1 and sha256[0].get('checksumValue') == expected[name],
        f'SPDX binary does not have exactly its qualified SHA-256: {name}',
    )
    reported[name] = expected[name]
require(reported == expected, 'SPDX does not describe exactly the eight qualified binary digests')
PY
