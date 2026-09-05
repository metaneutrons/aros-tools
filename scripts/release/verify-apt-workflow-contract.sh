#!/usr/bin/env bash
set -euo pipefail

repository_root=${1:-.}
python3 - "$repository_root" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
workflows = root / '.github/workflows'
errors = []
texts = {}
for name in ('release.yml', 'publish-ecosystem.yml'):
    path = workflows / name
    if not path.is_file() or path.is_symlink():
        errors.append(f'{name}: missing regular workflow')
    else:
        texts[name] = path.read_text()
if (workflows / 'refresh-apt-metadata.yml').exists():
    errors.append('tools-owned APT refresh remains; the central archive owns refresh')

for name, text in texts.items():
    for forbidden in (
        'secrets.APT_GPG_', 'secrets.R2_', 'environment: apt-signing',
        'environment: apt-publication', 'build-apt-repository.sh',
        'download-verify-apt-publication.sh', 'dists/stable',
    ):
        if forbidden in text:
            errors.append(f'{name}: obsolete archive ownership: {forbidden}')

def block(text, name):
    match = re.search(rf'(?ms)^  {re.escape(name)}:\n.*?(?=^  [a-z][a-z0-9-]*:\n|\Z)', text)
    return match.group() if match else ''

for workflow, name, mode in (
    ('release.yml', 'archive-credential-preflight', 'preflight'),
    ('publish-ecosystem.yml', 'apt', 'dispatch'),
):
    text = block(texts.get(workflow, ''), name)
    for marker in (
        'environment: apt-archive-publication',
        'actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1',
        'client-id: ${{ vars.ARCHIVE_DISPATCH_CLIENT_ID }}',
        'private-key: ${{ secrets.ARCHIVE_DISPATCH_PRIVATE_KEY }}',
        'owner: metaneutrons', 'repositories: apt-archive',
        'permission-actions: write', 'permission-contents: read',
        f'scripts/release/request-central-apt.sh {mode}',
    ):
        if marker not in text:
            errors.append(f'{workflow}/{name}: missing scoped request contract: {marker}')
    permissions = re.findall(r'^          permission-([a-z-]+): ([a-z]+)$', text, re.M)
    if sorted(permissions) != [('actions', 'write'), ('contents', 'read')]:
        errors.append(f'{workflow}/{name}: archive App token has excess permissions')
    if 'skip-token-revoke' in text:
        errors.append(f'{workflow}/{name}: ephemeral archive token revocation may not be disabled')

publication = texts.get('publish-ecosystem.yml', '')
for name, dependency in (('apt-verify', 'apt'), ('apt-install', 'apt-verify')):
    text = block(publication, name)
    for marker in (f'needs: {dependency}', 'verify-central-apt.py --mode exact',
                   '--candidate-dir candidate'):
        if marker not in text:
            errors.append(f'{name}: missing public consumer gate: {marker}')
    if 'secrets.' in text or re.search(r'^    environment:', text, re.M):
        errors.append(f'{name}: public verification must remain credential-free')
if 'verify-release-ref.sh' not in block(publication, 'apt') or '.immutable == true' not in block(publication, 'apt'):
    errors.append('central dispatch must follow immutable source-release validation')
if errors:
    for error in errors:
        print(f'::error::AP7238 {error}', file=sys.stderr)
    raise SystemExit(1)
print('validated central APT ownership, scoped dispatch and credential-free consumer gates')
PY
