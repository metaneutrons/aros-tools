#!/usr/bin/env bash

set -euo pipefail

repository_root=${1:-.}
workflow_root="$repository_root/.github/workflows"
[[ -d "$workflow_root" && ! -L "$workflow_root" ]] || {
    printf '%s\n' '::error::AP7238 workflow directory is missing or unsafe' >&2
    exit 1
}

python3 - "$workflow_root" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
publish_path = root / 'publish-ecosystem.yml'
refresh_path = root / 'refresh-apt-metadata.yml'
errors: list[str] = []

for path in (publish_path, refresh_path):
    if not path.is_file() or path.is_symlink():
        errors.append(f'{path}: APT workflow is missing or unsafe')

if errors:
    for error in errors:
        print(f'::error::AP7238 {error}', file=sys.stderr)
    raise SystemExit(1)

publish = publish_path.read_text()
refresh = refresh_path.read_text()

def require(text: str, marker: str, label: str) -> None:
    if marker not in text:
        errors.append(f'{label}: missing contract marker {marker!r}')

def function_body(text: str, name: str, label: str) -> str:
    match = re.search(
        rf'(?ms)^          {re.escape(name)}\(\) \{{\n(.*?)^          \}}$',
        text,
    )
    if match is None:
        errors.append(f'{label}: function {name} is missing or structurally ambiguous')
        return ''
    return match.group(1)

for forbidden in ('environment: apt-refresh-signing', 'environment: apt-refresh-publication'):
    if forbidden in refresh:
        errors.append(f'{refresh_path}: legacy environment remains: {forbidden}')
require(refresh, '    environment: apt-signing', str(refresh_path))
require(refresh, '    environment: apt-publication', str(refresh_path))

release_snapshot = function_body(publish, 'snapshot_mutable', str(publish_path))
release_put = function_body(publish, 'publish_snapshot', str(publish_path))
refresh_snapshot = function_body(refresh, 'snapshot_mutable', str(refresh_path))
refresh_put = function_body(refresh, 'put_snapshot', str(refresh_path))
for label, body in (
    ('release snapshot', release_snapshot),
    ('refresh snapshot', refresh_snapshot),
):
    require(body, 'head-object', label)
    require(body, 'get-object', label)
    require(body, '--if-match "$etag"', label)
for label, body in (('release put', release_put), ('refresh put', refresh_put)):
    if 'head-object' in body or 'get-object' in body:
        errors.append(f'{label}: mutation re-reads R2 instead of using the validated snapshot')
    require(body, '--if-match', label)
    require(body, "--if-none-match '*'", label)

for text, label in ((publish, str(publish_path)), (refresh, str(refresh_path))):
    for marker in (
        "snapshot_mutable 'aros-tools/dists/stable/Release' Release",
        "snapshot_mutable 'aros-tools/dists/stable/Release.gpg' Release-gpg",
        "snapshot_mutable 'aros-tools/dists/stable/InRelease' InRelease",
        'snapshot_mutable "aros-tools/${prefix}/Packages"',
        'snapshot_mutable "aros-tools/${prefix}/Packages.gz"',
    ):
        require(text, marker, label)

require(publish, 'download-verify-apt-publication.sh', str(publish_path))
require(publish, 'complete public APT inventory did not converge', str(publish_path))
require(refresh, 'verify-apt-recovery-base.sh', str(refresh_path))
require(refresh, 'download-verify-apt-publication.sh', str(refresh_path))
require(refresh, '    needs: [prepare, publish]', str(refresh_path))
require(refresh, 'complete refreshed APT inventory did not converge', str(refresh_path))
require(refresh, 'put_snapshot "$rebuilt/${prefix}/Packages"', str(refresh_path))
require(refresh, 'put_snapshot "$rebuilt/${prefix}/Packages.gz"', str(refresh_path))

release_snapshot_end = publish.find("snapshot_mutable 'aros-tools/dists/stable/InRelease' InRelease")
release_first_put = publish.find('aws s3api put-object', release_snapshot_end)
if release_snapshot_end < 0 or release_first_put < release_snapshot_end:
    errors.append(f'{publish_path}: release mutation can precede the complete mutable snapshot')
refresh_snapshot_end = refresh.find("snapshot_mutable 'aros-tools/dists/stable/InRelease' InRelease")
refresh_first_put = refresh.find('aws s3api put-object', refresh_snapshot_end)
if refresh_snapshot_end < 0 or refresh_first_put < refresh_snapshot_end:
    errors.append(f'{refresh_path}: refresh mutation can precede the complete mutable snapshot')

if errors:
    for error in errors:
        print(f'::error::AP7238 {error}', file=sys.stderr)
    raise SystemExit(1)
print('validated APT workflow trust, recovery, and snapshot-CAS contracts')
PY
