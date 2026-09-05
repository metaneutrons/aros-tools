#!/usr/bin/env bash

set -euo pipefail

repository_root=${1:-.}
workflow_root="$repository_root/.github/workflows"

[[ -d "$workflow_root" && ! -L "$workflow_root" ]] || {
    printf '::error::AP7040 workflow directory is missing or unsafe: %s\n' "$workflow_root" >&2
    exit 1
}

python3 - "$workflow_root" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
errors: list[str] = []
action = re.compile(r"^[^/@\s]+/[^@\s]+@[0-9a-f]{40}$")
digest = re.compile(r"^[^\s]+@sha256:[0-9a-f]{64}$")
trusted_actions = {
    'Homebrew/actions/setup-homebrew',
    'actions/attest-build-provenance',
    'actions/checkout',
    'actions/configure-pages',
    'actions/create-github-app-token',
    'actions/deploy-pages',
    'actions/download-artifact',
    'actions/setup-node',
    'actions/upload-artifact',
    'actions/upload-pages-artifact',
    'github/codeql-action/analyze',
    'github/codeql-action/init',
    'googleapis/release-please-action',
    'sigstore/cosign-installer',
}
local_actions = root.parent / 'actions'
for path in sorted((*root.glob('*.yml'), *root.glob('*.yaml'),
                    *local_actions.rglob('action.yml'), *local_actions.rglob('action.yaml'))):
    lines = path.read_text().splitlines()
    for line_number, line in enumerate(lines, 1):
        uses_match = re.match(r"^\s*-?\s*uses:\s*([^#]+?)\s*(?:#.*)?$", line)
        if uses_match:
            value = uses_match.group(1).strip(" '\"")
            if value.startswith('./'):
                continue
            if not action.fullmatch(value):
                errors.append(
                    f'{path}:{line_number}: external action is not pinned to one full commit SHA: {value}'
                )
            elif value.rsplit('@', 1)[0] not in trusted_actions:
                errors.append(
                    f'{path}:{line_number}: external action is not in the repository trust policy: {value}'
                )
            elif value.rsplit('@', 1)[0] == 'actions/checkout':
                indentation = len(line) - len(line.lstrip())
                block: list[str] = []
                for following in lines[line_number:]:
                    stripped = following.lstrip()
                    following_indentation = len(following) - len(stripped)
                    if stripped and following_indentation <= indentation and re.match(
                        r'-\s+(?:uses|name|run):', stripped
                    ):
                        break
                    block.append(following)
                invocation = '\n'.join(block)
                if re.search(r'^\s+persist-credentials:\s*false\s*$', invocation, re.MULTILINE) is None:
                    errors.append(
                        f'{path}:{line_number}: every checkout must set persist-credentials: false'
                    )
        image_match = re.match(r"^\s*(?:container|image):\s*([^#]+?)\s*(?:#.*)?$", line)
        if image_match:
            value = image_match.group(1).strip(" '\"")
            if not value or value.startswith('${{'):
                continue
            if not digest.fullmatch(value):
                errors.append(
                    f'{path}:{line_number}: container image is not pinned to one SHA-256 digest: {value}'
                )
        if 'gh attestation verify ' in line:
            invocation = '\n'.join(lines[line_number - 1:line_number + 10])
            for flag in (
                '--signer-workflow',
                '--source-ref',
                '--source-digest',
                '--deny-self-hosted-runners',
            ):
                if flag not in invocation:
                    errors.append(
                        f'{path}:{line_number}: attestation verification omits {flag}'
                    )

    if path.name == 'release.yml' and 'gh release create' in '\n'.join(lines):
        jobs: dict[str, str] = {}
        in_jobs = False
        current_name: str | None = None
        current: list[str] = []
        for line in lines:
            if line == 'jobs:':
                in_jobs = True
                continue
            if not in_jobs:
                continue
            match = re.match(r'^  ([a-z0-9][a-z0-9-]*):\s*$', line)
            if match:
                if current_name is not None:
                    jobs[current_name] = '\n'.join(current)
                current_name = match.group(1)
                current = [line]
            elif current_name is not None:
                current.append(line)
        if current_name is not None:
            jobs[current_name] = '\n'.join(current)

        recovery = jobs.get('release-recovery', '')
        for required in (
            'contents: write',
            '--paginate --slurp',
            '/releases?per_page=100',
            '/releases/${release_id}/assets?per_page=100',
            'recovered-release-state',
        ):
            if required not in recovery:
                errors.append(f'{path}: release recovery omits contract marker: {required}')
        for forbidden in ('id-token: write', 'actions/checkout@'):
            if forbidden in recovery:
                errors.append(f'{path}: release recovery must not contain {forbidden}')

        whole = '\n'.join(lines)
        for required in (
            '--notes-file candidate/RELEASE_NOTES.md',
            'missing-upload-order',
            'https://uploads.github.com/repos/${GITHUB_REPOSITORY}/releases/${release_id}/assets',
            'verify-publication-channels.sh',
            '--mode preflight',
            '--mode exact',
            'RELEASE_ADMIN_READ_TOKEN',
            'release-asset-metadata.py',
            'download-release-assets.sh',
            '--max-filesize',
        ):
            if required not in whole:
                errors.append(f'{path}: hardened release workflow omits {required}')
        if '--generate-notes' in whole:
            errors.append(f'{path}: generated release notes bypass the signed deterministic body')
        for forbidden in ('gh release upload', 'gh release edit'):
            if forbidden in whole:
                errors.append(
                    f'{path}: tag-addressed release mutation is forbidden: {forbidden}'
                )
        for forbidden in ('gh release download', 'read_bytes('):
            if forbidden in whole:
                errors.append(
                    f'{path}: release body retrieval bypasses bounded API metadata: {forbidden}'
                )
        if whole.count('RELEASE_ADMIN_READ_TOKEN') != 2:
            errors.append(
                f'{path}: administration-read credential must occur only in one env binding and one check'
            )

        secret_domains = {
            'homebrew': ('secrets.HOMEBREW_APP_PRIVATE_KEY',),
            'archive': ('secrets.ARCHIVE_DISPATCH_PRIVATE_KEY',),
            'r2': ('secrets.R2_ACCESS_KEY_ID', 'secrets.R2_SECRET_ACCESS_KEY'),
            'apt': ('secrets.APT_GPG_PRIVATE_KEY', 'secrets.APT_GPG_PASSPHRASE'),
            'aur': ('secrets.AUR_SSH_PRIVATE_KEY',),
            'admin': ('secrets.RELEASE_ADMIN_READ_TOKEN',),
        }
        for job_name, block in jobs.items():
            present = {
                domain
                for domain, markers in secret_domains.items()
                if any(marker in block for marker in markers)
            }
            if len(present) > 1:
                errors.append(
                    f'{path}: job {job_name} mixes secret domains: {sorted(present)}'
                )
            if 'contents: write' in block and 'id-token: write' in block:
                errors.append(
                    f'{path}: job {job_name} combines repository mutation with OIDC signing'
                )

        expected_environments = {
            'release-config-preflight': 'release',
            'homebrew-credential-preflight': 'homebrew-publication',
            'archive-credential-preflight': 'apt-archive-publication',
            'aur-credential-preflight': 'aur-publication',
        }
        for job_name, environment in expected_environments.items():
            block = jobs.get(job_name, '')
            measured = re.findall(r'^    environment:\s*([^\s#]+)', block, re.MULTILINE)
            if measured != [environment]:
                errors.append(
                    f'{path}: job {job_name} must bind only environment {environment}'
                )
            for required in ('trap cleanup EXIT', "trap 'exit 130' HUP INT TERM"):
                if required not in block:
                    errors.append(
                        f'{path}: secret preflight {job_name} lacks cleanup marker {required}'
                    )
            if job_name != 'release-config-preflight' and (
                "needs.release-config-preflight.result == 'success'" not in block
                or 'release-config-preflight' not in block
            ):
                errors.append(
                    f'{path}: secret preflight {job_name} can run before validated configuration'
                )
        homebrew_preflight = jobs.get('homebrew-credential-preflight', '')
        homebrew_install = jobs.get('homebrew', '')
        matrix_source = '\n'.join(line for line in homebrew_install.splitlines()
                                  if not line.lstrip().startswith('#'))
        measured_hosts = re.findall(
            r'- name: (\S+)\n\s+runner: (\S+)\n\s+target: (\S+)', matrix_source)
        if measured_hosts != [
            ('linux-x86_64', 'ubuntu-24.04', 'x86_64-unknown-linux-gnu'),
            ('linux-aarch64', 'ubuntu-24.04-arm', 'aarch64-unknown-linux-gnu'),
            ('macos-x86_64', 'macos-15-intel', 'x86_64-apple-darwin'),
            ('macos-aarch64', 'macos-15', 'aarch64-apple-darwin'),
        ]:
            errors.append(f'{path}: Homebrew install matrix must bind four genuine native hosts')
        for required in (
            'verify-homebrew-install.py host', 'verify-homebrew-install.py installed',
            "brew ruby -e 'puts Hardware::CPU.arch'", '--brew-prefix "$(brew --prefix)"',
            '--target "$TARGET"', '--manifest "candidate/aros-tools-v${VERSION}-${TARGET}.tar.gz.manifest.json"',
            'if ! brew install --verbose "$tap/aros-tools"; then',
            'AP7322', 'brew test "$tap/aros-tools"',
        ):
            if required not in homebrew_install:
                errors.append(f'{path}: Homebrew install qualification omits {required}')
        if (
            'verify-branch-protection.sh' not in homebrew_preflight
            or '--repository metaneutrons/homebrew-tap' not in homebrew_preflight
        ):
            errors.append(
                f'{path}: Homebrew credential preflight does not enforce the tap governance SSOT'
            )
        for job_name in ('channel-preflight', 'publication-preflight'):
            block = jobs.get(job_name, '')
            if re.search(r'^    environment:', block, re.MULTILINE):
                errors.append(f'{path}: credential-free job {job_name} binds an environment')
            if 'secrets.' in block:
                errors.append(f'{path}: credential-free job {job_name} references a secret')

release_scripts = root.parent.parent / 'scripts' / 'release'
transport_paths = [
    *sorted(root.glob('*.yml')),
    *sorted(root.glob('*.yaml')),
    *(path for path in sorted(release_scripts.glob('*.sh'))
      if path.name != 'test-release-policy.sh'),
]
for path in transport_paths:
    lines = path.read_text().splitlines()
    for line_number, line in enumerate(lines, 1):
        # YAML `container:` coverage is insufficient: a shell step can invoke
        # Docker/Podman directly and silently reintroduce a mutable image tag.
        # Treat the bounded multiline invocation as one policy unit and demand
        # a literal digest. Variable-selected images are deliberately rejected.
        if re.search(r'\b(?:docker|podman)\s+(?:[^\s]+\s+)*?(?:run|pull)\b', line):
            invocation = '\n'.join(lines[line_number - 1:line_number + 12])
            if not re.search(r'(?<![A-Za-z0-9_])[^\s"\']+@sha256:[0-9a-f]{64}(?=[\s"\']|$)', invocation):
                errors.append(
                    f'{path}:{line_number}: shell container invocation has no literal SHA-256 image digest'
                )
        # Accept long or short curl options and direct/variable URLs as an
        # invocation.  A policy that noticed only `curl --...` could be bypassed
        # with `curl -L ...` or `curl "$url"`.
        if re.search(r'''\bcurl\s+(?:--?|["'$]|https?://)''', line) is None:
            continue
        invocation = '\n'.join(lines[line_number - 1:line_number + 7])
        if 'http://127.0.0.1' in invocation:
            continue
        for flag in ("--proto '=https'", '--tlsv1.2'):
            if flag not in invocation:
                errors.append(f'{path}:{line_number}: curl transport omits {flag}')
        if '--location' in invocation and "--proto-redir '=https'" not in invocation:
            errors.append(
                f"{path}:{line_number}: redirect-following curl omits --proto-redir '=https'"
            )

# Private publication credentials are separate trust domains.  A job is the
# smallest enforceable GitHub-hosted runner boundary, so no job may combine
# two domains and every secret-bearing step must install fail-safe cleanup.
secret_patterns = {
    'apt-archive-publication': re.compile(r"\$\{\{\s*secrets\.ARCHIVE_DISPATCH_PRIVATE_KEY"),
    'apt-signing': re.compile(r"\$\{\{\s*secrets\.APT_GPG_"),
    'r2-publication': re.compile(r"\$\{\{\s*secrets\.R2_"),
    'aur-publication': re.compile(r"\$\{\{\s*secrets\.AUR_"),
    'homebrew-publication': re.compile(r"\$\{\{\s*secrets\.HOMEBREW_APP_PRIVATE_KEY"),
    'docs-publication': re.compile(r"\$\{\{\s*secrets\.CLOUDFLARE_API_TOKEN"),
}

def workflow_jobs(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    lines = path.read_text().splitlines()
    try:
        start = lines.index('jobs:') + 1
    except ValueError:
        return {}
    starts: list[tuple[str, int]] = []
    for index in range(start, len(lines)):
        match = re.fullmatch(r"  ([A-Za-z0-9_-]+):", lines[index])
        if match:
            starts.append((match.group(1), index))
    result: dict[str, str] = {}
    for position, (name, index) in enumerate(starts):
        end = starts[position + 1][1] if position + 1 < len(starts) else len(lines)
        result[name] = '\n'.join(lines[index:end])
    return result

def job_steps(job: str) -> list[str]:
    lines = job.splitlines()
    starts = [index for index, line in enumerate(lines)
              if re.match(r"^      - (?:name:|uses:)", line)]
    return [
        '\n'.join(lines[index:(starts[position + 1]
                                 if position + 1 < len(starts) else len(lines))])
        for position, index in enumerate(starts)
    ]

for workflow_name in ('publish-ecosystem.yml',):
    path = root / workflow_name
    if not path.exists():
        continue
    jobs = workflow_jobs(path)
    workflow_text = path.read_text()
    if 'gh release download' in workflow_text or 'read_bytes(' in workflow_text:
        errors.append(f'{path}: unbounded release body retrieval is forbidden')
    for job_name, job in jobs.items():
        domains = {name for name, pattern in secret_patterns.items() if pattern.search(job)}
        if len(domains) > 1:
            errors.append(
                f'{path}: job {job_name} combines private credential domains: '
                + ', '.join(sorted(domains))
            )
        for step in job_steps(job):
            step_domains = {name for name, pattern in secret_patterns.items()
                            if pattern.search(step)}
            if not step_domains:
                continue
            # The pinned token action revokes its short-lived installation token
            # in its post step. There is no plaintext key file or shell cleanup.
            if (step_domains == {'apt-archive-publication'} and
                'actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1' in step and
                'repositories: apt-archive' in step and
                'permission-actions: write' in step and
                'permission-contents: read' in step and
                'skip-token-revoke' not in step):
                continue
            if (step_domains == {'homebrew-publication'} and
                'uses: ./.github/actions/homebrew-token' in step and
                'client-id: ${{ vars.HOMEBREW_APP_CLIENT_ID }}' in step and
                'private-key: ${{ secrets.HOMEBREW_APP_PRIVATE_KEY }}' in step):
                # This local composite is checked below, including post-revocation.
                continue
            for required in (
                'trap cleanup EXIT',
                "trap 'exit 130' HUP INT TERM",
                '--kill gpg-agent',
            ):
                if required not in step:
                    errors.append(
                        f'{path}: secret-bearing step in job {job_name} lacks {required}'
                    )

publish_jobs = workflow_jobs(root / 'publish-ecosystem.yml')
release_jobs = workflow_jobs(root / 'release.yml')
rp_path = root / 'release-please.yml'
if rp_path.exists():
    rp_job = workflow_jobs(rp_path).get('release-pr', '')
    required_rp = (
        'environment: release-please',
        "if: github.ref == 'refs/heads/main'",
        'actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1',
        'client-id: ${{ vars.RELEASE_PLEASE_CLIENT_ID }}',
        'private-key: ${{ secrets.RELEASE_PLEASE_APP_PRIVATE_KEY }}',
        'token: ${{ steps.app-token.outputs.token }}',
        'GH_TOKEN: ${{ steps.app-token.outputs.token }}',
        'installation/repositories?per_page=100',
        '--paginate --slurp',
        'skip-github-release: true',
        'scripts/validate-version-contract.py',
    )
    for marker in required_rp:
        if marker not in rp_job:
            errors.append(f'{rp_path}: missing Release Please App contract: {marker}')
    for field, value in (('owner', 'metaneutrons'), ('repositories', 'aros-tools')):
        if re.findall(rf'^\s+{field}:\s*([^\n]+)', rp_job, re.MULTILINE) != [value]:
            errors.append(f'{rp_path}: Release Please App target must be exactly {field}: {value}')
    if re.findall(r'^\s+(permission-[\w-]+):\s*(\S+)', rp_job, re.MULTILINE) != [
        ('permission-contents', 'write'), ('permission-pull-requests', 'write'),
    ]:
        errors.append(f'{rp_path}: Release Please App must request only Contents and Pull requests write')
    for forbidden in ('github.token', 'secrets.GITHUB_TOKEN', 'skip-token-revoke',
                      'gh workflow run', 'actions: write', 'issues: write'):
        if forbidden in rp_job:
            errors.append(f'{rp_path}: forbidden Release Please credential or duplicate dispatch: {forbidden}')
    if set(re.findall(r'secrets\.([A-Z_]+)', rp_job)) != {'RELEASE_PLEASE_APP_PRIVATE_KEY'}:
        errors.append(f'{rp_path}: Release Please must use only its own private key')

# The local composite is the sole Homebrew credential factory. Its underlying
# pinned action revokes every token at job end (including failure/cancellation).
homebrew_action = local_actions / 'homebrew-token' / 'action.yml'
homebrew_users = [
    job for jobs in (publish_jobs, release_jobs) for job in jobs.values()
    if 'HOMEBREW_APP_PRIVATE_KEY' in job or './.github/actions/homebrew-token' in job
]
if homebrew_users:
    factory = homebrew_action.read_text() if homebrew_action.is_file() else ''
    required_factory = (
        'using: composite',
        'uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1',
        'client-id: ${{ inputs.client-id }}', 'private-key: ${{ inputs.private-key }}',
        'owner: metaneutrons', 'repositories: homebrew-tap',
        'permission-contents: write', 'permission-pull-requests: write',
        'permission-actions: read', 'permission-checks: read',
        'permission-statuses: read', 'permission-administration: read',
        'GH_TOKEN: ${{ steps.app.outputs.token }}',
        'HOMEBREW_APP_SLUG: ${{ steps.app.outputs.app-slug }}',
        'HOMEBREW_INSTALLATION_ID: ${{ steps.app.outputs.installation-id }}',
        'scripts/release/verify-homebrew-app.sh',
    )
    for required in required_factory:
        if required not in factory:
            errors.append(f'{homebrew_action}: missing isolated App contract: {required}')
    for field, value in (('owner', 'metaneutrons'), ('repositories', 'homebrew-tap')):
        if re.findall(rf'^\s+{field}:\s*([^\n]+)', factory, re.MULTILINE) != [value]:
            errors.append(f'{homebrew_action}: credential factory target must be exactly {field}: {value}')
    permissions = re.findall(r'^\s+(permission-[\w-]+):\s*(\S+)', factory, re.MULTILINE)
    if dict(permissions) != {
        'permission-contents': 'write', 'permission-pull-requests': 'write',
        'permission-actions': 'read', 'permission-checks': 'read',
        'permission-statuses': 'read', 'permission-administration': 'read',
    } or len(permissions) != 6:
        errors.append(f'{homebrew_action}: unexpected or duplicate permission grant')
    for forbidden in ('skip-token-revoke', 'permission-workflows', 'github-api-url:'):
        if forbidden in factory:
            errors.append(f'{homebrew_action}: forbidden credential-factory override: {forbidden}')
    for job in homebrew_users:
        if '    environment: homebrew-publication' not in job:
            errors.append('Homebrew App key must stay inside homebrew-publication')
        for step in job_steps(job):
            if 'HOMEBREW_APP_PRIVATE_KEY' in step and (
                'uses: ./.github/actions/homebrew-token' not in step or '\n        run:' in step
            ):
                errors.append('Homebrew private key must only reach the verified token factory')

for path in (*root.glob('*.yml'), *root.glob('*.yaml')):
    for legacy in ('HOMEBREW_TAP_TOKEN', 'PACKAGE_PUBLISH_TOKEN'):
        if legacy in path.read_text():
            errors.append(f'{path}: legacy Homebrew PAT binding is forbidden: {legacy}')

docs_jobs = workflow_jobs(root / 'docs.yml')
docs_build = docs_jobs.get('build', '')
docs_deploy = docs_jobs.get('deploy', '')
if docs_jobs:
    if 'secrets.' in docs_build or re.search(r'^    environment:', docs_build, re.MULTILINE):
        errors.append(f'{root / "docs.yml"}: documentation build must remain credential-free')
    if docs_deploy.count('secrets.CLOUDFLARE_API_TOKEN') != 1 or docs_deploy.count('secrets.') != 1:
        errors.append(
            f'{root / "docs.yml"}: deploy must receive only one Cloudflare documentation secret'
        )
    for required in (
        'npm run worker:deploy',
        'trap cleanup EXIT',
        "trap 'exit 130' HUP INT TERM",
        'https://aros.metaneutrons.cc/aros-tools/',
    ):
        if required not in docs_deploy:
            errors.append(f'{root / "docs.yml"}: deploy omits contract marker: {required}')
    for forbidden in ('contents: write', 'id-token: write', 'secrets.R2_'):
        if forbidden in docs_deploy:
            errors.append(f'{root / "docs.yml"}: deploy must not contain {forbidden}')
homebrew_job = publish_jobs.get('homebrew', '')
if (root / 'publish-ecosystem.yml').exists() and (
    'verify_tap_governance()' not in homebrew_job
    or homebrew_job.count('verify_tap_governance') < 4
    or 'scripts/release/verify-branch-protection.sh' not in homebrew_job
    or '--repository metaneutrons/homebrew-tap' not in homebrew_job
):
    errors.append(
        f'{root / "publish-ecosystem.yml"}: Homebrew mutation and merge do not revalidate the governance SSOT'
    )
if homebrew_job:
    steps = job_steps(homebrew_job)
    wait = next((i for i, step in enumerate(steps) if '--watch --fail-fast' in step), -1)
    renew = next((i for i, step in enumerate(steps) if 'id: homebrew-merge-token' in step), -1)
    merge_step = next((i for i, step in enumerate(steps) if 'gh pr merge' in step), -1)
    if not 0 <= wait < renew < merge_step:
        errors.append('Homebrew must renew its App token between qualification wait and merge')
    else:
        if ('timeout-minutes: 35' not in steps[wait]
                or 'python3 scripts/release/wait-homebrew-checks.py' not in steps[wait]
                or 'EXPECTED_HEAD: ${{ steps.update.outputs.head_sha }}' not in steps[wait]
                or 'timeout-minutes: 10' not in steps[merge_step]
                or 'gh pr checks' not in steps[merge_step]
                or 'GH_TOKEN: ${{ steps.homebrew-merge-token.outputs.token }}' not in steps[merge_step]
                or 'uses: ./.github/actions/homebrew-token' not in steps[renew]):
            errors.append('Homebrew wait/merge must be bounded and use a newly verified App token')
    update = next((step for step in steps if 'id: update' in step), '')
    if ('timeout-minutes: 10' not in update
            or 'GH_TOKEN: ${{ steps.homebrew-token.outputs.token }}' not in update
            or 'git config user.name "$BOT_NAME"' not in update
            or 'git config user.email "$BOT_EMAIL"' not in update
            or 'gh api user ' in update):
        errors.append('Homebrew update must use the fresh App token and verified bot identity')
    if 'GH_TOKEN: ${{ steps.homebrew-merge-token.outputs.token }}' not in steps[-1]:
        errors.append('Homebrew final read-back must use the renewed tap App token')
    if 'reviewDecision' in homebrew_job or 'independent approval' in homebrew_job:
        errors.append(
            f'{root / "publish-ecosystem.yml"}: solo-maintainer Homebrew publication must not wait for a self-review'
        )
    if '--match-head-commit "$EXPECTED_HEAD"' not in homebrew_job:
        errors.append(
            f'{root / "publish-ecosystem.yml"}: Homebrew merge lacks the exact-head precondition'
        )
    for mutation in ('git push ', 'gh pr create'):
        for match in re.finditer(re.escape(mutation), homebrew_job):
            verifier = homebrew_job.rfind('verify_tap_governance', 0, match.start())
            if verifier < 0 or match.start() - verifier > 1400:
                errors.append(
                    f'{root / "publish-ecosystem.yml"}: {mutation.strip()} is not immediately guarded by tap governance'
                )
    merge = homebrew_job.find('gh pr merge')
    direct_verifier = homebrew_job.rfind(
        'scripts/release/verify-branch-protection.sh', 0, merge
    )
    if merge < 0 or direct_verifier < 0 or merge - direct_verifier > 400:
        errors.append(
            f'{root / "publish-ecosystem.yml"}: Homebrew merge is not directly guarded by tap governance'
        )
expected_domains = {
    ('docs.yml', 'build'): set(),
    ('docs.yml', 'deploy'): {'docs-publication'},
    ('publish-ecosystem.yml', 'apt'): {'apt-archive-publication'},
    ('publish-ecosystem.yml', 'homebrew'): {'homebrew-publication'},
    ('publish-ecosystem.yml', 'aur-publish'): {'aur-publication'},
    ('publish-ecosystem.yml', 'aur-verify'): set(),
}
for (workflow_name, job_name), expected in expected_domains.items():
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
    }[workflow_name]
    job = jobs.get(job_name)
    if job is None:
        errors.append(f'{root / workflow_name}: required isolated job is missing: {job_name}')
        continue
    measured = {name for name, pattern in secret_patterns.items() if pattern.search(job)}
    if measured != expected:
        errors.append(
            f'{root / workflow_name}: job {job_name} credential domain is '
            f'{sorted(measured)}, expected {sorted(expected)}'
        )

expected_environments = {
    ('docs.yml', 'deploy'): 'docs-publication',
    ('publish-ecosystem.yml', 'apt'): 'apt-archive-publication',
    ('publish-ecosystem.yml', 'homebrew'): 'homebrew-publication',
    ('publish-ecosystem.yml', 'aur-publish'): 'aur-publication',
}
for (workflow_name, job_name), environment in expected_environments.items():
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
    }[workflow_name]
    job = jobs.get(job_name, '')
    if f'    environment: {environment}' not in job:
        errors.append(
            f'{workflow_path}: job {job_name} must use isolated environment {environment}'
        )

credential_free_jobs = {
    ('docs.yml', 'build'),
    ('publish-ecosystem.yml', 'apt-verify'),
    ('publish-ecosystem.yml', 'apt-install'),
    ('publish-ecosystem.yml', 'aur-verify'),
}
for workflow_name, job_name in credential_free_jobs:
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
    }[workflow_name]
    job = jobs.get(job_name, '')
    if re.search(r'^    environment:', job, re.MULTILINE):
        errors.append(
            f'{workflow_path}: credential-free job {job_name} must not enter an environment'
        )

handoff_contracts: list[tuple[str, str]] = []
if (root / 'publish-ecosystem.yml').exists():
    handoff_contracts.extend((
        (publish_jobs.get('aur-publish', ''), 'name: aur-publication-evidence'),
        (publish_jobs.get('aur-verify', ''), 'name: aur-publication-evidence'),
    ))
# Project publication must never regain the central archive's private keys or
# storage permissions, even if a new job happens to isolate those credentials.
for path in sorted(root.glob('*.yml')):
    source = path.read_text()
    for forbidden in ('secrets.APT_GPG_', 'secrets.R2_'):
        if forbidden in source:
            errors.append(f'{path}: central archive credential is forbidden here: {forbidden}')
if (root / 'refresh-apt-metadata.yml').exists():
    errors.append('tools-owned APT refresh must not coexist with the central archive')
for job, marker in handoff_contracts:
    if marker not in job:
        errors.append(f'{root}: isolated publication handoff is missing marker {marker}')

if errors:
    for error in errors:
        print(f'::error::{error}', file=sys.stderr)
    raise SystemExit(1)
print(f'validated trusted immutable action and container references in {root}')
PY
