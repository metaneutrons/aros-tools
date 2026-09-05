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
for path in sorted((*root.glob('*.yml'), *root.glob('*.yaml'))):
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
            'homebrew': ('secrets.HOMEBREW_TAP_TOKEN',),
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
            'r2-credential-preflight': 'apt-publication',
            'apt-credential-preflight': 'apt-signing',
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
        if (
            'verify-branch-protection.sh' not in homebrew_preflight
            or '--repository metaneutrons/homebrew-tap' not in homebrew_preflight
        ):
            errors.append(
                f'{path}: Homebrew credential preflight does not enforce the tap governance SSOT'
            )
        if '--kill gpg-agent' not in jobs.get('apt-credential-preflight', ''):
            errors.append(f'{path}: APT credential preflight does not terminate gpg-agent')
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
    'apt-signing': re.compile(r"\$\{\{\s*secrets\.APT_GPG_"),
    'r2-publication': re.compile(r"\$\{\{\s*secrets\.R2_"),
    'aur-publication': re.compile(r"\$\{\{\s*secrets\.AUR_"),
    'homebrew-publication': re.compile(r"\$\{\{\s*secrets\.HOMEBREW_TAP_TOKEN"),
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

for workflow_name in ('publish-ecosystem.yml', 'refresh-apt-metadata.yml'):
    path = root / workflow_name
    if not path.exists():
        continue
    jobs = workflow_jobs(path)
    workflow_text = path.read_text()
    if workflow_name == 'refresh-apt-metadata.yml' and 'download-release-assets.sh' not in workflow_text:
        errors.append(
            f'{path}: refresh does not use the bounded API-first release downloader'
        )
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
refresh_jobs = workflow_jobs(root / 'refresh-apt-metadata.yml')
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
    ('publish-ecosystem.yml', 'apt-sign'): {'apt-signing'},
    ('publish-ecosystem.yml', 'apt'): {'r2-publication'},
    ('publish-ecosystem.yml', 'homebrew'): {'homebrew-publication'},
    ('publish-ecosystem.yml', 'aur-publish'): {'aur-publication'},
    ('publish-ecosystem.yml', 'aur-verify'): set(),
    ('refresh-apt-metadata.yml', 'prepare'): set(),
    ('refresh-apt-metadata.yml', 'sign'): {'apt-signing'},
    ('refresh-apt-metadata.yml', 'publish'): {'r2-publication'},
    ('refresh-apt-metadata.yml', 'verify'): set(),
}
for (workflow_name, job_name), expected in expected_domains.items():
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
        'refresh-apt-metadata.yml': refresh_jobs,
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
    ('publish-ecosystem.yml', 'apt-sign'): 'apt-signing',
    ('publish-ecosystem.yml', 'apt'): 'apt-publication',
    ('publish-ecosystem.yml', 'homebrew'): 'homebrew-publication',
    ('publish-ecosystem.yml', 'aur-publish'): 'aur-publication',
    ('refresh-apt-metadata.yml', 'sign'): 'apt-signing',
    ('refresh-apt-metadata.yml', 'publish'): 'apt-publication',
}
for (workflow_name, job_name), environment in expected_environments.items():
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
        'refresh-apt-metadata.yml': refresh_jobs,
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
    ('refresh-apt-metadata.yml', 'prepare'),
    ('refresh-apt-metadata.yml', 'verify'),
}
for workflow_name, job_name in credential_free_jobs:
    workflow_path = root / workflow_name
    if not workflow_path.exists():
        continue
    jobs = {
        'docs.yml': docs_jobs,
        'publish-ecosystem.yml': publish_jobs,
        'refresh-apt-metadata.yml': refresh_jobs,
    }[workflow_name]
    job = jobs.get(job_name, '')
    if re.search(r'^    environment:', job, re.MULTILINE):
        errors.append(
            f'{workflow_path}: credential-free job {job_name} must not enter an environment'
        )

handoff_contracts: list[tuple[str, str]] = []
if (root / 'publish-ecosystem.yml').exists():
    handoff_contracts.extend((
        (publish_jobs.get('apt-sign', ''), 'name: signed-apt-publication'),
        (publish_jobs.get('apt', ''), 'name: signed-apt-publication'),
        (publish_jobs.get('aur-publish', ''), 'name: aur-publication-evidence'),
        (publish_jobs.get('aur-verify', ''), 'name: aur-publication-evidence'),
    ))
if (root / 'refresh-apt-metadata.yml').exists():
    handoff_contracts.extend((
        (refresh_jobs.get('prepare', ''), 'name: apt-refresh-unsigned-release'),
        (refresh_jobs.get('sign', ''), 'name: apt-refresh-unsigned-release'),
        (refresh_jobs.get('sign', ''), 'name: signed-apt-refresh'),
        (refresh_jobs.get('publish', ''), 'name: signed-apt-refresh'),
        (refresh_jobs.get('verify', ''), 'name: signed-apt-refresh'),
    ))
for job, marker in handoff_contracts:
    if marker not in job:
        errors.append(f'{root}: isolated publication handoff is missing marker {marker}')

if errors:
    for error in errors:
        print(f'::error::{error}', file=sys.stderr)
    raise SystemExit(1)
print(f'validated trusted immutable action and container references in {root}')
PY
