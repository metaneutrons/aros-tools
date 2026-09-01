#!/usr/bin/env bash

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/aros-release-policy.XXXXXX")
fixture_gnupg=
revoked_gnupg=
expired_gnupg=
cleanup() {
    for home in "$fixture_gnupg" "$revoked_gnupg" "$expired_gnupg"; do
        if [[ -n "$home" && -d "$home" ]]; then
            rm -rf -- "$home"
        fi
    done
    rm -rf -- "$work"
}
trap cleanup EXIT

expect_failure() {
    if "$@" >"$work/unexpected.stdout" 2>"$work/unexpected.stderr"; then
        printf 'expected failure but command succeeded:' >&2
        printf ' %q' "$@" >&2
        printf '\n' >&2
        exit 1
    fi
}

expect_failure_matching() {
    local pattern=$1
    shift
    expect_failure "$@"
    grep -F "$pattern" "$work/unexpected.stderr" >/dev/null || {
        printf 'expected failure did not contain %q:\n' "$pattern" >&2
        sed 's/^/  /' "$work/unexpected.stderr" >&2
        exit 1
    }
}

# CI treats cross-repository contracts as bounded inert data, installs its
# pinned action linter into an explicit path, and resolves declared Debian
# runtime dependencies before exercising the package installation boundary.
python3 - "$root/.github/workflows/ci.yml" \
    "$root/.github/workflows/release.yml" \
    "$root/scripts/release" <<'PY'
import pathlib
import re
import sys

ci = open(sys.argv[1], encoding='utf-8').read()
release = open(sys.argv[2], encoding='utf-8').read()
required_ci = (
    'Retrieve the exact producer workflow as bounded inert data',
    'GOBIN="$actionlint_dir"',
    'github.com/rhysd/actionlint/cmd/actionlint@v1.7.12',
    '"$actionlint_dir/actionlint" -version',
)
if any(marker not in ci for marker in required_ci):
    raise SystemExit('CI qualification hardening contract is incomplete')
if (
    'repository: ${{ steps.contract.outputs.producer_repository }}' in ci
    or 'git -C aros-producer' in ci
):
    raise SystemExit('CI executes a dynamic cross-repository producer checkout')
dependency_install = 'apt-get install --yes --no-install-recommends'
package_install = 'dpkg --install "$deb"'
if dependency_install not in release or release.index(dependency_install) > release.index(package_install):
    raise SystemExit('Debian runtime dependencies are not resolved before package installation')
arch_dependencies = 'pacman --sync --refresh --sysupgrade --noconfirm --needed'
if arch_dependencies not in release or release.index(arch_dependencies) > release.index('makepkg --config'):
    raise SystemExit('Arch runtime dependencies are not resolved before package construction')
if "cancel-in-progress: ${{ github.ref_type != 'tag' }}" not in release:
    raise SystemExit('release qualification does not cancel superseded non-tag runs')

release_scripts = pathlib.Path(sys.argv[3])
for script in release_scripts.glob('*.sh'):
    for number, line in enumerate(script.read_text(encoding='utf-8').splitlines(), 1):
        if re.match(r'^[A-Za-z_][A-Za-z0-9_]*=\$\(', line) is None:
            continue
        if 'dirname' not in line or '$0' not in line or 'pwd' not in line:
            continue
        cdpath_free = 'CDPATH= cd --' in line or 'unset CDPATH; cd --' in line
        if not cdpath_free or 'dirname --' not in line or 'pwd -P' not in line:
            raise SystemExit(
                f'{script.name}:{number}: script-root derivation is not physical and CDPATH-free'
            )
PY

# Every public-output helper delegates parent creation to one no-follow policy.
# Existing caller-owned modes are preserved exactly and a symlink parent is
# rejected before any output is created.
mode_parent="$work/output-parent"
mkdir "$mode_parent"
chmod 0700 "$mode_parent"
before_mode=$(python3 - "$mode_parent" <<'PY'
import os, stat, sys
print(f'{stat.S_IMODE(os.lstat(sys.argv[1]).st_mode):04o}')
PY
)
"$root/scripts/release/prepare-output-parent.sh" \
    --path "$mode_parent/new-output" --mode 0755
after_mode=$(python3 - "$mode_parent" <<'PY'
import os, stat, sys
print(f'{stat.S_IMODE(os.lstat(sys.argv[1]).st_mode):04o}')
PY
)
[[ "$before_mode" == 0700 && "$after_mode" == "$before_mode" ]] || {
    printf '%s\n' 'output-parent helper changed a caller-owned directory mode' >&2
    exit 1
}
mkdir "$work/output-parent-target"
ln -s "$work/output-parent-target" "$work/output-parent-link"
expect_failure "$root/scripts/release/prepare-output-parent.sh" \
    --path "$work/output-parent-link/new-output" --mode 0755
for helper in \
    verify-apt-public-key.sh download-bounded-https.sh \
    download-release-assets.sh download-verify-apt-publication.sh \
    build-apt-repository.sh; do
    grep -F 'prepare-output-parent.sh' "$root/scripts/release/$helper" >/dev/null || {
        printf 'public-output helper bypasses parent policy: %s\n' "$helper" >&2
        exit 1
    }
done

# The mutable repository variable, checked-in trust anchor and installation
# documentation are one identity contract.
production_fingerprint=$(tr -d '\r\n' < "$root/contracts/apt-trust-anchor.txt")
"$root/scripts/release/verify-apt-trust-anchor.sh" \
    --fingerprint "$production_fingerprint" \
    --documentation "$root/docs-site/src/content/docs/getting-started/installation.md"
expect_failure "$root/scripts/release/verify-apt-trust-anchor.sh" \
    --fingerprint 0000000000000000000000000000000000000000
grep -F 'verify-apt-trust-anchor.sh' "$root/.github/workflows/release.yml" >/dev/null
grep -F 'verify-apt-trust-anchor.sh' \
    "$root/.github/workflows/publish-ecosystem.yml" >/dev/null
grep -F 'verify-apt-trust-anchor.sh' \
    "$root/.github/workflows/refresh-apt-metadata.yml" >/dev/null

# Release mutation window: exact boundary succeeds, stale/future identities fail.
AROS_RELEASE_NOW_EPOCH=1700000000 \
    "$root/scripts/release/verify-release-window.sh" \
        --tag-date-epoch 1699395200 >/dev/null
expect_failure env AROS_RELEASE_NOW_EPOCH=1700000000 \
    "$root/scripts/release/verify-release-window.sh" \
        --tag-date-epoch 1699395199
expect_failure env AROS_RELEASE_NOW_EPOCH=1700000000 \
    "$root/scripts/release/verify-release-window.sh" \
        --tag-date-epoch 1700000301

# The release body is exactly one canonical CHANGELOG section, rendered
# byte-identically and rejected when the section is missing or ambiguous.
cat > "$work/CHANGELOG.md" <<'MARKDOWN'
# Changelog

## [1.3.0](https://example.invalid/compare/v1.2.3...v1.3.0) (2024-01-08)

### Features

* deterministic notes

## [1.2.3](https://example.invalid/compare/v1.2.2...v1.2.3) (2024-01-01)

### Bug Fixes

* exact release body

## [1.2.2](https://example.invalid/compare/v1.2.1...v1.2.2)

* previous release
MARKDOWN
for copy in a b; do
    "$root/scripts/release/render-release-notes.py" \
        --changelog "$work/CHANGELOG.md" --version 1.2.3 \
        --output "$work/RELEASE_NOTES-${copy}.md"
done
cmp "$work/RELEASE_NOTES-a.md" "$work/RELEASE_NOTES-b.md"
cat > "$work/expected-release-notes.md" <<'MARKDOWN'
## [1.2.3](https://example.invalid/compare/v1.2.2...v1.2.3) (2024-01-01)

### Bug Fixes

* exact release body
MARKDOWN
cmp "$work/expected-release-notes.md" "$work/RELEASE_NOTES-a.md"
expect_failure "$root/scripts/release/render-release-notes.py" \
    --changelog "$work/CHANGELOG.md" --version 9.9.9 \
    --output "$work/missing-release-notes.md"
printf '\n## 1.2.3\n\n* duplicate\n' >> "$work/CHANGELOG.md"
expect_failure "$root/scripts/release/render-release-notes.py" \
    --changelog "$work/CHANGELOG.md" --version 1.2.3 \
    --output "$work/duplicate-release-notes.md"

# Remote release identity and active immutable-tag policy are tested with a
# hermetic gh transport; no repository state is read or changed.
mkdir "$work/mock-bin"
cat > "$work/mock-bin/gh" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]] || exit 90
shift
if [[ "${1:-}" == -H ]]; then
    shift 2
fi
if [[ "${1:-}" == --paginate ]]; then
    shift
    [[ "${1:-}" == --slurp ]] || exit 91
    shift
    case "$1" in
        repos/example/project/rulesets*)
            printf '%s\n' '[[{"id":7,"target":"tag","enforcement":"active"}]]'
            ;;
        repos/example/project/releases/99/assets*)
            if [[ "${MOCK_BAD_ASSET:-}" == 1 ]]; then
                jq -c '.[0].digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000" | [.]' \
                    "${MOCK_ASSET_METADATA:?}"
            else
                jq -c '[.]' "${MOCK_ASSET_METADATA:?}"
            fi
            ;;
        *) printf 'unexpected paginated mock gh endpoint: %s\n' "$1" >&2; exit 91 ;;
    esac
    exit 0
fi
case "$1" in
    repos/example/project/git/ref/tags/v1.2.3)
        printf '%s\n' '{"object":{"type":"tag","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}'
        ;;
    repos/example/project/git/tags/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)
        printf '%s\n' '{"object":{"type":"commit","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"tagger":{"date":"2024-01-01T00:00:00Z"}}'
        ;;
    repos/example/project/rulesets/7)
        case "${MOCK_BAD_RULESET:-}" in
            missing-deletion)
                printf '%s\n' '{"target":"tag","enforcement":"active","bypass_actors":[],"conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},"rules":[{"type":"update"}]}'
                ;;
            bypass)
                printf '%s\n' '{"target":"tag","enforcement":"active","bypass_actors":[{"actor_id":1,"actor_type":"OrganizationAdmin","bypass_mode":"always"}],"conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},"rules":[{"type":"update"},{"type":"deletion"}]}'
                ;;
            wrong-pattern)
                printf '%s\n' '{"target":"tag","enforcement":"active","bypass_actors":[],"conditions":{"ref_name":{"include":["refs/tags/release-*"],"exclude":[]}},"rules":[{"type":"update"},{"type":"deletion"}]}'
                ;;
            *)
                printf '%s\n' '{"target":"tag","enforcement":"active","bypass_actors":[],"conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},"rules":[{"type":"update"},{"type":"deletion"}]}'
                ;;
        esac
        ;;
    repos/example/project/branches/main)
        printf '%s\n' '{"protected":true,"commit":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}'
        ;;
    repos/example/project/branches/main/protection)
        base='{"required_status_checks":{"strict":true,"checks":[{"context":"CI / quality","app_id":15368}]},"required_pull_request_reviews":{"required_approving_review_count":1,"dismiss_stale_reviews":true,"require_code_owner_reviews":true,"require_last_push_approval":true},"required_conversation_resolution":{"enabled":true},"enforce_admins":{"enabled":true},"required_linear_history":{"enabled":true},"allow_force_pushes":{"enabled":false},"allow_deletions":{"enabled":false}}'
        case "${MOCK_BAD_MAIN_PROTECTION:-}" in
            non-strict) jq -c '.required_status_checks.strict = false' <<<"$base" ;;
            admin-bypass) jq -c '.enforce_admins.enabled = false' <<<"$base" ;;
            nonlinear) jq -c '.required_linear_history.enabled = false' <<<"$base" ;;
            force-push) jq -c '.allow_force_pushes.enabled = true' <<<"$base" ;;
            deletion) jq -c '.allow_deletions.enabled = true' <<<"$base" ;;
            *) printf '%s\n' "$base" ;;
        esac
        ;;
    repos/example/project/releases/tags/v1.2.3)
        if [[ "${MOCK_RELEASE_MUTABLE:-}" == 1 ]]; then
            printf '%s\n' '{"id":99,"tag_name":"v1.2.3","draft":false,"prerelease":false,"immutable":false}'
        else
            printf '%s\n' '{"id":99,"tag_name":"v1.2.3","draft":false,"prerelease":false,"immutable":true}'
        fi
        ;;
    *) printf 'unexpected mock gh endpoint: %s\n' "$1" >&2; exit 92 ;;
esac
MOCK
chmod 0755 "$work/mock-bin/gh"
cat > "$work/governance.toml" <<'TOML'
schema_version = 1

[repositories."example/project"]
branch = "main"
required_approving_review_count = 1
dismiss_stale_reviews = true
require_code_owner_reviews = true
require_last_push_approval = true
required_conversation_resolution = true
enforce_admins = true
required_linear_history = true
allow_force_pushes = false
allow_deletions = false
required_status_checks = [
  { context = "CI / quality", app_id = 15368 },
]
TOML
verify_fixture_ref=(
    "$root/scripts/release/verify-release-ref.sh"
    --repository example/project --tag v1.2.3
    --tag-object aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    --source-commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
    --tag-date-epoch 1704067200
    --governance-contract "$work/governance.toml"
)
expect_failure env GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "${verify_fixture_ref[@]}"
AROS_RELEASE_POLICY_FIXTURE=1 GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "${verify_fixture_ref[@]}" >/dev/null
for invalid_ruleset in missing-deletion bypass wrong-pattern; do
    expect_failure env MOCK_BAD_RULESET="$invalid_ruleset" \
        AROS_RELEASE_POLICY_FIXTURE=1 GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
        "${verify_fixture_ref[@]}"
done
for invalid_protection in non-strict admin-bypass nonlinear force-push deletion; do
    expect_failure env MOCK_BAD_MAIN_PROTECTION="$invalid_protection" \
        AROS_RELEASE_POLICY_FIXTURE=1 GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
        "${verify_fixture_ref[@]}"
done

# Published release assets are accepted only as the exact 48-file signed
# inventory with API-bound IDs, states, type caps, sizes and streaming SHA-256.
mkdir "$work/assets"
if command -v sha256sum >/dev/null; then
    fixture_checksum=(sha256sum)
else
    fixture_checksum=(shasum -a 256)
fi
python3 "$root/scripts/release/release-asset-metadata.py" contract \
    --version 1.2.3 > "$work/asset-contract.json"
python3 - "$work/asset-contract.json" "$work/assets" \
    "$work/asset-metadata.json" <<'PY'
import hashlib
import json
import pathlib
import sys

contract = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
directory = pathlib.Path(sys.argv[2])
metadata = []
for identifier, name in enumerate(contract['assets'], start=1):
    path = directory / name
    path.write_bytes((name + '\n').encode())
    with path.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    metadata.append({
        'id': identifier,
        'name': name,
        'state': 'uploaded',
        'size': path.stat().st_size,
        'digest': f'sha256:{digest}',
    })
pathlib.Path(sys.argv[3]).write_text(json.dumps(metadata), encoding='utf-8')
PY
MOCK_ASSET_METADATA="$work/asset-metadata.json" \
    GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "$root/scripts/release/verify-release-assets.sh" \
        --repository example/project --tag v1.2.3 \
        --candidate-dir "$work/assets" >/dev/null
printf '%s\n' extra > "$work/assets/extra.bin"
expect_failure env MOCK_ASSET_METADATA="$work/asset-metadata.json" \
    GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "$root/scripts/release/verify-release-assets.sh" \
      --repository example/project --tag v1.2.3 \
      --candidate-dir "$work/assets"
unlink "$work/assets/extra.bin"
expect_failure env MOCK_BAD_ASSET=1 \
    MOCK_ASSET_METADATA="$work/asset-metadata.json" \
    GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "$root/scripts/release/verify-release-assets.sh" \
        --repository example/project --tag v1.2.3 \
        --candidate-dir "$work/assets"
expect_failure env MOCK_RELEASE_MUTABLE=1 \
    MOCK_ASSET_METADATA="$work/asset-metadata.json" \
    GH_TOKEN=fixture PATH="$work/mock-bin:$PATH" \
    "$root/scripts/release/verify-release-assets.sh" \
        --repository example/project --tag v1.2.3 \
        --candidate-dir "$work/assets"

# Per-type and metadata-body limits are enforced before any release body is
# fetched.  Mutating only the API metadata cannot authorize oversized content.
python3 - "$work/asset-contract.json" "$work/asset-metadata.json" \
    "$work/asset-oversized.json" <<'PY'
import json
import pathlib
import sys

contract = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
metadata = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding='utf-8'))
metadata[0]['size'] = contract['assets'][metadata[0]['name']] + 1
pathlib.Path(sys.argv[3]).write_text(json.dumps(metadata), encoding='utf-8')
PY
expect_failure "$root/scripts/release/release-asset-metadata.py" validate \
    --version 1.2.3 --metadata-json "$work/asset-oversized.json" --mode exact
mkdir "$work/download-mock-bin"
cp "$work/mock-bin/gh" "$work/download-mock-bin/gh"
cat > "$work/download-mock-bin/curl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
: > "${MOCK_CURL_CALLED:?}"
exit 99
MOCK
chmod 0755 "$work/download-mock-bin/gh" "$work/download-mock-bin/curl"
expect_failure env MOCK_ASSET_METADATA="$work/asset-oversized.json" \
    MOCK_CURL_CALLED="$work/curl-called-before-metadata" GH_TOKEN=fixture \
    PATH="$work/download-mock-bin:$PATH" \
    "$root/scripts/release/download-release-assets.sh" \
      --repository example/project --release-id 99 --version 1.2.3 \
      --directory "$work/rejected-download" --mode exact
[[ ! -e "$work/curl-called-before-metadata" ]] || {
    printf '%s\n' 'release downloader fetched a body before rejecting API metadata' >&2
    exit 1
}
python3 - "$work/metadata-too-large.json" <<'PY'
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_bytes(b' ' * (1024 * 1024 + 1))
PY
expect_failure "$root/scripts/release/release-asset-metadata.py" validate \
    --version 1.2.3 --metadata-json "$work/metadata-too-large.json" --mode exact
python3 - "$work/apt-key-too-large.asc" <<'PY'
import pathlib
import sys

pathlib.Path(sys.argv[1]).write_bytes(b'x' * (1024 * 1024 + 1))
PY
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/download-bounded-https.sh" \
      --source-file "$work/apt-key-too-large.asc" \
      --output "$work/oversized-key.output" --class apt-key

# Recovery never replaces a valid historical keyless bundle.  It verifies and
# copies the exact bundle only when the reproduced subject is byte-identical.
cat > "$work/mock-bin/cosign" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
    verify-blob)
        bundle=
        while (($#)); do
            [[ "$1" == --bundle ]] && { bundle=$2; shift 2; continue; }
            shift
        done
        [[ -f "$bundle" ]] || exit 88
        ;;
    sign-blob)
        bundle=
        while (($#)); do
            [[ "$1" == --bundle ]] && { bundle=$2; shift 2; continue; }
            shift
        done
        printf '%s\n' signed > "$bundle"
        ;;
    *) exit 89 ;;
esac
MOCK
chmod 0755 "$work/mock-bin/cosign"
mkdir "$work/recovery-candidate" "$work/recovery-existing"
printf '%s\n' payload > "$work/recovery-candidate/artifact"
cp "$work/recovery-candidate/artifact" "$work/recovery-existing/artifact"
printf '%s\n' historical-bundle > "$work/recovery-existing/artifact.sigstore.json"
PATH="$work/mock-bin:$PATH" "$root/scripts/release/reuse-sigstore-bundles.sh" \
    --candidate-dir "$work/recovery-candidate" \
    --existing-dir "$work/recovery-existing" \
    --certificate-identity \
      'https://github.com/example/project/.github/workflows/release.yml@refs/tags/v1.2.3' \
    --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
    --allow-new false >/dev/null
cmp "$work/recovery-existing/artifact.sigstore.json" \
    "$work/recovery-candidate/artifact.sigstore.json"
printf '%s\n' changed > "$work/recovery-candidate/artifact"
expect_failure env PATH="$work/mock-bin:$PATH" \
    "$root/scripts/release/reuse-sigstore-bundles.sh" \
      --candidate-dir "$work/recovery-candidate" \
      --existing-dir "$work/recovery-existing" \
      --certificate-identity \
        'https://github.com/example/project/.github/workflows/release.yml@refs/tags/v1.2.3' \
      --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
        --allow-new false

# A/B classification uses only immutable published stable releases and a
# closed low-risk path policy. Source-only patches may use one producer;
# release/build-graph changes and unavailable history require full A/B.
mkdir -p "$work/ab-repo/crates/aros-cli/src"
git -C "$work/ab-repo" init -q
git -C "$work/ab-repo" config user.name fixture
git -C "$work/ab-repo" config user.email fixture@example.invalid
cat > "$work/ab-repo/Cargo.toml" <<'TOML'
[package]
name = "aros-cli"
version = "1.2.2"
TOML
cat > "$work/ab-repo/Cargo.lock" <<'TOML'
version = 3

[[package]]
name = "aros-cli"
version = "1.2.2"
TOML
printf '%s\n' 'fn main() {}' > "$work/ab-repo/crates/aros-cli/src/main.rs"
printf '%s\n' '# Changelog' > "$work/ab-repo/CHANGELOG.md"
git -C "$work/ab-repo" add .
git -C "$work/ab-repo" commit -qm 'v1.2.2'
git -C "$work/ab-repo" tag -a v1.2.2 -m v1.2.2
sed -i.bak 's/1\.2\.2/1.2.3/g' "$work/ab-repo/Cargo.toml" "$work/ab-repo/Cargo.lock"
rm "$work/ab-repo/Cargo.toml.bak" "$work/ab-repo/Cargo.lock.bak"
printf '%s\n' 'fn main() { println!("patch"); }' \
    > "$work/ab-repo/crates/aros-cli/src/main.rs"
printf '%s\n' '## 1.2.3' >> "$work/ab-repo/CHANGELOG.md"
git -C "$work/ab-repo" add .
git -C "$work/ab-repo" commit -qm 'v1.2.3'
git -C "$work/ab-repo" tag -a v1.2.3 -m v1.2.3
cat > "$work/releases-1.2.3.json" <<'JSON'
[[{"tag_name":"v1.2.2","draft":false,"prerelease":false,"immutable":true}]]
JSON
ab_state=$(cd "$work/ab-repo" && AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/classify-release-ab.py" \
      --repository example/project --tag v1.2.3 \
      --source-commit "$(git rev-parse 'v1.2.3^{}')" \
      --releases-json "$work/releases-1.2.3.json")
jq -e '.requires_ab == false and .reason == "closed-low-risk-patch"' \
    <<< "$ab_state" >/dev/null
mkdir -p "$work/ab-repo/scripts/release"
printf '%s\n' '# producer change' > "$work/ab-repo/scripts/release/producer.sh"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$work/ab-repo/Cargo.toml" "$work/ab-repo/Cargo.lock"
rm "$work/ab-repo/Cargo.toml.bak" "$work/ab-repo/Cargo.lock.bak"
git -C "$work/ab-repo" add .
git -C "$work/ab-repo" commit -qm 'v1.2.4'
git -C "$work/ab-repo" tag -a v1.2.4 -m v1.2.4
cat > "$work/releases-1.2.4.json" <<'JSON'
[[{"tag_name":"v1.2.3","draft":false,"prerelease":false,"immutable":true}]]
JSON
ab_state=$(cd "$work/ab-repo" && AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/classify-release-ab.py" \
      --repository example/project --tag v1.2.4 \
      --source-commit "$(git rev-parse 'v1.2.4^{}')" \
      --releases-json "$work/releases-1.2.4.json")
jq -e '.requires_ab == true and (.reason | startswith("unclassified-change:"))' \
    <<< "$ab_state" >/dev/null
mkdir -p "$work/ab-repo/contracts"
printf '%s\n' 'schema_version = 2' > "$work/ab-repo/contracts/policy.toml"
sed -i.bak 's/1\.2\.4/1.2.5/g' "$work/ab-repo/Cargo.toml" "$work/ab-repo/Cargo.lock"
rm "$work/ab-repo/Cargo.toml.bak" "$work/ab-repo/Cargo.lock.bak"
git -C "$work/ab-repo" add .
git -C "$work/ab-repo" commit -qm 'v1.2.5'
git -C "$work/ab-repo" tag -a v1.2.5 -m v1.2.5
cat > "$work/releases-1.2.5.json" <<'JSON'
[[{"tag_name":"v1.2.4","draft":false,"prerelease":false,"immutable":true}]]
JSON
ab_state=$(cd "$work/ab-repo" && AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/classify-release-ab.py" \
      --repository example/project --tag v1.2.5 \
      --source-commit "$(git rev-parse 'v1.2.5^{}')" \
      --releases-json "$work/releases-1.2.5.json")
jq -e '.requires_ab == true and .reason == "unclassified-change:contracts/policy.toml"' \
    <<< "$ab_state" >/dev/null
ab_state=$(cd "$work/ab-repo" && \
    "$root/scripts/release/classify-release-ab.py" \
      --repository example/project --tag v1.2.5 \
      --source-commit "$(git rev-parse 'v1.2.5^{}')" \
      --releases-json "$work/releases-1.2.5.json")
jq -e '.requires_ab == true and (.reason | startswith("release-history-unavailable:"))' \
    <<< "$ab_state" >/dev/null

# Closed inventories reject both missing and unexpected release assets, and
# SHA256SUMS must name every pre-checksum subject exactly once.
mkdir "$work/closed-inventory"
inventory_names="$work/inventory-names"
: > "$inventory_names"
for target in \
    aarch64-apple-darwin aarch64-unknown-linux-gnu \
    x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    archive="aros-tools-v1.2.3-${target}.tar.gz"
    for name in "$archive" "${archive}.manifest.json" \
        "${archive}.sha256" "aros-tools-v1.2.3-${target}.spdx.json"; do
        printf '%s\n' "$name" >> "$inventory_names"
    done
done
for arch in amd64 arm64; do
    printf '%s\n' "aros-tools_1.2.3_${arch}.deb" \
        "aros-tools_1.2.3_${arch}.spdx.json" >> "$inventory_names"
done
printf '%s\n' PKGBUILD aros-tools.rb >> "$inventory_names"
while IFS= read -r name; do
    : > "$work/closed-inventory/$name"
    printf '%064d  ./%s\n' 0 "$name"
done < "$inventory_names" > "$work/closed-inventory/SHA256SUMS"
"$root/scripts/release/verify-candidate-inventory.sh" \
    --directory "$work/closed-inventory" --version 1.2.3 --signed false >/dev/null
: > "$work/closed-inventory/unexpected"
expect_failure "$root/scripts/release/verify-candidate-inventory.sh" \
    --directory "$work/closed-inventory" --version 1.2.3 --signed false
unlink "$work/closed-inventory/unexpected"
cp -R "$work/closed-inventory" "$work/closed-signed-inventory"
rm "$work/closed-signed-inventory/SHA256SUMS"
printf '%s\n' '## 1.2.3' '' '* signed notes' \
    > "$work/closed-signed-inventory/RELEASE_NOTES.md"
find "$work/closed-signed-inventory" -mindepth 1 -maxdepth 1 -type f \
    -exec basename {} \; | LC_ALL=C sort > "$work/signed-subjects"
while IFS= read -r name; do
    printf '%s\n' bundle > "$work/closed-signed-inventory/${name}.sigstore.json"
done < "$work/signed-subjects"
find "$work/closed-signed-inventory" -mindepth 1 -maxdepth 1 -type f \
    -exec basename {} \; | LC_ALL=C sort | \
    awk '{ printf "%064d  ./%s\n", 0, $0 }' \
    > "$work/signed-SHA256SUMS"
mv "$work/signed-SHA256SUMS" "$work/closed-signed-inventory/SHA256SUMS"
printf '%s\n' bundle > "$work/closed-signed-inventory/SHA256SUMS.sigstore.json"
"$root/scripts/release/verify-candidate-inventory.sh" \
    --directory "$work/closed-signed-inventory" \
    --version 1.2.3 --signed true >/dev/null

# Build one tiny but structurally exact release archive and SPDX fixture. The
# verifier must accept it and reject digest, inventory and schema corruption.
python3 - "$work" <<'PY'
import hashlib
import json
import pathlib
import tarfile
import sys

work = pathlib.Path(sys.argv[1])
payload = work / 'payload' / 'aros-tools-v1.2.3-test' / 'bin'
payload.mkdir(parents=True)
names = [
    'aros', 'aros-ahi-runner', 'aros-collect', 'aros-fetch',
    'aros-genmodule', 'aros-romtool', 'aros-transpiler', 'aros-verify',
]
for name in names:
    path = payload / name
    path.write_bytes((name + '\n').encode())
    path.chmod(0o755)
archive = work / 'aros-tools-v1.2.3-test.tar.gz'
with tarfile.open(archive, 'w:gz') as output:
    output.add(payload.parent, arcname=payload.parent.name)
artifact_digest = hashlib.sha256(archive.read_bytes()).hexdigest()
root_id = 'SPDXRef-Package-root'
files = []
for index, name in enumerate(names):
    digest = hashlib.sha256((payload / name).read_bytes()).hexdigest()
    files.append({
        'fileName': f'bin/{name}',
        'SPDXID': f'SPDXRef-File-{index}',
        'checksums': [{'algorithm': 'SHA256', 'checksumValue': digest}],
        'licenseConcluded': 'NOASSERTION',
        'copyrightText': 'NOASSERTION',
    })
document = {
    'spdxVersion': 'SPDX-2.3',
    'dataLicense': 'CC0-1.0',
    'SPDXID': 'SPDXRef-DOCUMENT',
    'name': archive.name,
    'documentNamespace': f'https://aros.metaneutrons.cc/spdx/aros-tools/{artifact_digest}',
    'creationInfo': {'created': '2024-01-01T00:00:00Z', 'creators': ['Tool: syft-1.51.1']},
    'packages': [{
        'name': archive.name,
        'SPDXID': root_id,
        'versionInfo': '1.2.3',
        'packageFileName': archive.name,
        'downloadLocation': 'NOASSERTION',
        'filesAnalyzed': False,
        'licenseConcluded': 'NOASSERTION',
        'licenseDeclared': 'NOASSERTION',
        'copyrightText': 'NOASSERTION',
        'checksums': [{'algorithm': 'SHA256', 'checksumValue': artifact_digest}],
    }],
    'files': files,
    'relationships': [{
        'spdxElementId': 'SPDXRef-DOCUMENT',
        'relationshipType': 'DESCRIBES',
        'relatedSpdxElement': root_id,
    }],
}
(work / 'valid.spdx.json').write_text(json.dumps(document), encoding='utf-8')
bad_digest = json.loads(json.dumps(document))
bad_digest['files'][0]['checksums'][0]['checksumValue'] = '0' * 64
(work / 'bad-digest.spdx.json').write_text(json.dumps(bad_digest), encoding='utf-8')
bad_inventory = json.loads(json.dumps(document))
bad_inventory['files'].append({
    'fileName': 'bin/evil', 'SPDXID': 'SPDXRef-File-evil',
    'checksums': [{'algorithm': 'SHA256', 'checksumValue': '1' * 64}],
})
(work / 'bad-inventory.spdx.json').write_text(json.dumps(bad_inventory), encoding='utf-8')
bad_schema = json.loads(json.dumps(document))
bad_schema['spdxVersion'] = 'SPDX-2.2'
(work / 'bad-schema.spdx.json').write_text(json.dumps(bad_schema), encoding='utf-8')
bad_ambiguous_digest = json.loads(json.dumps(document))
bad_ambiguous_digest['files'][0]['checksums'].append(
    {'algorithm': 'SHA256', 'checksumValue': '2' * 64}
)
(work / 'bad-ambiguous-digest.spdx.json').write_text(
    json.dumps(bad_ambiguous_digest), encoding='utf-8'
)
(work / 'artifact.sha256').write_text(artifact_digest, encoding='ascii')
PY
artifact="$work/aros-tools-v1.2.3-test.tar.gz"
digest=$(cat "$work/artifact.sha256")
verify=(
    "$root/scripts/release/verify-spdx-sbom.sh"
    --artifact "$artifact" --expected-sha256 "$digest"
    --kind archive --version 1.2.3
)
"${verify[@]}" --sbom "$work/valid.spdx.json"
expect_failure "${verify[@]}" --sbom "$work/bad-digest.spdx.json"
expect_failure "${verify[@]}" --sbom "$work/bad-inventory.spdx.json"
expect_failure "${verify[@]}" --sbom "$work/bad-schema.spdx.json"
expect_failure "${verify[@]}" --sbom "$work/bad-ambiguous-digest.spdx.json"

# APT metadata is rendered by repository-owned standard-library code.  Two
# builds at one epoch are byte-identical; a protected refresh changes only the
# signed release triplet while package, Packages and by-hash bytes stay fixed.
mkdir "$work/apt-candidate"
python3 - "$work/apt-candidate" <<'PY'
import gzip
import io
import pathlib
import shutil
import sys
import tarfile

root = pathlib.Path(sys.argv[1])

def tar_gz(name: str, payload: bytes) -> bytes:
    output = io.BytesIO()
    with gzip.GzipFile(filename='', mode='wb', mtime=0, fileobj=output) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as archive:
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            info.mtime = 0
            info.uid = info.gid = 0
            info.uname = info.gname = ''
            archive.addfile(info, io.BytesIO(payload))
    return output.getvalue()

def ar_member(name: str, payload: bytes) -> bytes:
    header = (
        f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{0o100644:<8o}{len(payload):<10}`\n'
    ).encode('ascii')
    return header + payload + (b'\n' if len(payload) % 2 else b'')

for arch in ('amd64', 'arm64'):
    control = (
        'Package: aros-tools\nVersion: 1.2.3-1\n'
        f'Architecture: {arch}\nMaintainer: Test <test@example.invalid>\n'
        'Description: deterministic fixture\n'
    ).encode()
    package = b'!<arch>\n'
    package += ar_member('debian-binary', b'2.0\n')
    package += ar_member('control.tar.gz', tar_gz('./control', control))
    package += ar_member('data.tar.gz', tar_gz('./usr/share/aros-tools-fixture', b'ok\n'))
    (root / f'aros-tools_1.2.3_{arch}.deb').write_bytes(package)

# A tiny compressed control member must not be allowed to expand without a
# hard bound. Keep the valid arm64 package so the renderer reaches amd64 input
# validation through the same exact two-architecture contract.
bomb_root = root.parent / 'apt-bomb-candidate'
bomb_root.mkdir()
for arch in ('amd64', 'arm64'):
    shutil.copy2(
        root / f'aros-tools_1.2.3_{arch}.deb',
        bomb_root / f'aros-tools_1.2.3_{arch}.deb',
    )
bomb_control = io.BytesIO()
with gzip.GzipFile(filename='', mode='wb', mtime=0, fileobj=bomb_control) as compressed:
    with tarfile.open(fileobj=compressed, mode='w') as archive:
        info = tarfile.TarInfo('./padding')
        info.size = 16 * 1024 * 1024 + 1
        info.mtime = 0
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        archive.addfile(info, io.BytesIO(b'\0' * info.size))
bomb = b'!<arch>\n'
bomb += ar_member('debian-binary', b'2.0\n')
bomb += ar_member('control.tar.gz', bomb_control.getvalue())
bomb += ar_member('data.tar.gz', tar_gz('./fixture', b'ok\n'))
(bomb_root / 'aros-tools_1.2.3_amd64.deb').write_bytes(bomb)
PY
mkdir "$work/apt-bomb-output"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
    "$root/scripts/release/run-apt-metadata-renderer.sh" \
      "$work/apt-bomb-candidate" "$work/apt-bomb-output" 1.2.3 1704067200
grep -F -- '--memory 256m --memory-swap 256m --pids-limit 64 --cpus 2' \
    "$root/scripts/release/run-apt-metadata-renderer.sh" >/dev/null
# macOS has a short AF_UNIX path limit for gpg-agent sockets. Keep this fixture
# home under /tmp even when TMPDIR itself is a long per-user path.
fixture_gnupg=$(mktemp -d /tmp/aros-release-gpg.XXXXXX)
chmod 0700 "$fixture_gnupg"
export GNUPGHOME="$fixture_gnupg"
gpg --batch --faked-system-time '1704067200!' --pinentry-mode loopback \
    --passphrase '' --quick-generate-key \
    'AROS fixture <fixture@example.invalid>' rsa2048 sign 0 >/dev/null
apt_fingerprint=$(gpg --batch --with-colons --list-secret-keys --fingerprint | \
    awk -F: '$1 == "fpr" { print toupper($10); exit }')
gpg --batch --armor --pinentry-mode loopback --passphrase '' \
    --export-secret-keys "$apt_fingerprint" > "$work/apt-private.asc"
: > "$work/apt-passphrase"
for copy in a b; do
    AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
      "$root/scripts/release/build-apt-repository.sh" \
        --candidate-dir "$work/apt-candidate" \
        --output-dir "$work/apt-${copy}" --version 1.2.3 \
        --source-date-epoch 1704067200 \
        --private-key "$work/apt-private.asc" \
        --passphrase-file "$work/apt-passphrase" \
        --fingerprint "$apt_fingerprint"
done
diff --recursive --no-dereference "$work/apt-a" "$work/apt-b"

# One signing subkey per archive domain. This fixture covers the path the other
# cases never touch, because they call without --signing-subkey: the right
# subkey accepted, a foreign subkey rejected, and a bundle that still carries
# the secret primary key rejected.
subkey_gnupg=$(mktemp -d /tmp/aros-subkey-gpg.XXXXXX)
chmod 0700 "$subkey_gnupg"
GNUPGHOME="$subkey_gnupg" gpg --batch --faked-system-time '1704067200!' \
    --pinentry-mode loopback --passphrase '' --quick-generate-key \
    'AROS subkey fixture <subkey@example.invalid>' ed25519 cert 0 >/dev/null
subkey_primary=$(GNUPGHOME="$subkey_gnupg" gpg --batch --with-colons \
    --list-keys --fingerprint | awk -F: '$1 == "fpr" { print toupper($10); exit }')
for _ in 1 2; do
    GNUPGHOME="$subkey_gnupg" gpg --batch --faked-system-time '1704067200!' \
        --pinentry-mode loopback --passphrase '' \
        --quick-add-key "$subkey_primary" ed25519 sign 0 >/dev/null
done
mapfile -t subkeys < <(GNUPGHOME="$subkey_gnupg" gpg --batch --with-colons \
    --list-keys --fingerprint | \
    awk -F: '$1 == "sub" { want = 1; next } $1 == "fpr" && want { print toupper($10); want = 0 }')
[[ ${#subkeys[@]} == 2 ]] || {
    printf 'subkey fixture must expose exactly two signing subkeys\n' >&2
    exit 1
}
subkey_a=${subkeys[0]}
subkey_b=${subkeys[1]}
GNUPGHOME="$subkey_gnupg" gpg --batch --armor --pinentry-mode loopback \
    --passphrase '' --export-secret-subkeys "${subkey_a}!" > "$work/subkey-only.asc"
GNUPGHOME="$subkey_gnupg" gpg --batch --armor --pinentry-mode loopback \
    --passphrase '' --export-secret-keys "$subkey_primary" > "$work/subkey-full.asc"

AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
  "$root/scripts/release/build-apt-repository.sh" \
    --candidate-dir "$work/apt-candidate" --output-dir "$work/apt-subkey" \
    --version 1.2.3 --source-date-epoch 1704067200 \
    --private-key "$work/subkey-only.asc" \
    --passphrase-file "$work/apt-passphrase" \
    --fingerprint "$subkey_primary" --signing-subkey "$subkey_a"

# The shipped certificate must be minimised to exactly this subkey.
[[ $(gpg --no-options --batch --with-colons --show-keys \
        "$work/apt-subkey/aros-tools-archive-keyring.asc" | grep -c '^sub') == 1 ]] || {
    printf 'domain keyring must carry exactly one signing subkey\n' >&2
    exit 1
}
gpg --no-options --batch --dearmor \
    < "$work/apt-subkey/aros-tools-archive-keyring.asc" > "$work/subkey-keyring.gpg"
gpgv --keyring "$work/subkey-keyring.gpg" --status-fd 3 \
    "$work/apt-subkey/dists/stable/Release.gpg" \
    "$work/apt-subkey/dists/stable/Release" 3> "$work/subkey.status" 2>/dev/null
"$root/scripts/release/verify-gpgv-status.sh" --status-file "$work/subkey.status" \
    --fingerprint "$subkey_primary" --signing-subkey "$subkey_a"
# Derselbe Primaerschluessel, aber der Domain-Subkey passt nicht: ohne die
# Pruefung von VALIDSIG-Feld 1 bliebe das unsichtbar.
expect_failure "$root/scripts/release/verify-gpgv-status.sh" \
    --status-file "$work/subkey.status" \
    --fingerprint "$subkey_primary" --signing-subkey "$subkey_b"

expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
  "$root/scripts/release/build-apt-repository.sh" \
    --candidate-dir "$work/apt-candidate" --output-dir "$work/apt-subkey-wrong" \
    --version 1.2.3 --source-date-epoch 1704067200 \
    --private-key "$work/subkey-only.asc" \
    --passphrase-file "$work/apt-passphrase" \
    --fingerprint "$subkey_primary" --signing-subkey "$subkey_b"
# The secret primary key must never sit in the signing environment.
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
  "$root/scripts/release/build-apt-repository.sh" \
    --candidate-dir "$work/apt-candidate" --output-dir "$work/apt-subkey-full" \
    --version 1.2.3 --source-date-epoch 1704067200 \
    --private-key "$work/subkey-full.asc" \
    --passphrase-file "$work/apt-passphrase" \
    --fingerprint "$subkey_primary" --signing-subkey "$subkey_a"
rm -rf -- "$subkey_gnupg"
"$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-a" --mode full --version 1.2.3 \
    --fingerprint "$apt_fingerprint" >/dev/null
# A caller-controlled GnuPG option file must not change the canonical export or
# the measured trust-anchor identity.
printf '%s\n' 'emit-version' 'comment release-policy-injection' \
    > "$fixture_gnupg/gpg.conf"
"$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-a" --mode full --version 1.2.3 \
    --fingerprint "$apt_fingerprint" >/dev/null
unlink "$fixture_gnupg/gpg.conf"

# GnuPG can report VALIDSIG and return success for a signature whose primary
# key is now revoked. Both the canonical trust-anchor check and the shared
# status parser must reject that state explicitly.
revoked_gnupg=$(mktemp -d /tmp/aros-revoked-gpg.XXXXXX)
revoked_home=$revoked_gnupg
chmod 0700 "$revoked_home"
gpg --no-options --batch --homedir "$revoked_home" \
    --import "$work/apt-private.asc" >/dev/null 2>&1
sed 's/^://' "$fixture_gnupg/openpgp-revocs.d/${apt_fingerprint}.rev" | \
    gpg --no-options --batch --homedir "$revoked_home" --import >/dev/null 2>&1
gpg --no-options --batch --homedir "$revoked_home" --armor --no-emit-version \
    --no-comments --export "$apt_fingerprint" > "$work/revoked-key.asc"
expect_failure "$root/scripts/release/verify-apt-public-key.sh" \
    --key "$work/revoked-key.asc" --fingerprint "$apt_fingerprint" \
    --keyring-output "$work/revoked-keyring.gpg"
gpg --no-options --batch --homedir "$revoked_home" --yes --dearmor \
    --output "$work/revoked-status-keyring.gpg" "$work/revoked-key.asc"
set +e
gpgv --status-fd 3 --keyring "$work/revoked-status-keyring.gpg" \
    "$work/apt-a/dists/stable/InRelease" \
    3> "$work/revoked-inrelease.status" 2>/dev/null
set -e
grep -F '[GNUPG:] REVKEYSIG ' "$work/revoked-inrelease.status" >/dev/null
grep -F '[GNUPG:] VALIDSIG ' "$work/revoked-inrelease.status" >/dev/null
expect_failure "$root/scripts/release/verify-gpgv-status.sh" \
    --status-file "$work/revoked-inrelease.status" --fingerprint "$apt_fingerprint"
cp -R "$work/apt-a" "$work/apt-revoked-key"
install -m 0644 "$work/revoked-key.asc" \
    "$work/apt-revoked-key/aros-tools-archive-keyring.asc"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-revoked-copy" --source-directory "$work/apt-revoked-key" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

# Expired primary keys are rejected even when the key packet and armor are
# otherwise canonical. A real gpgv transcript (when supported by the host
# version) or the equivalent explicit status fixture must also fail closed.
expired_gnupg=$(mktemp -d /tmp/aros-expired-gpg.XXXXXX)
expired_home=$expired_gnupg
chmod 0700 "$expired_home"
gpg --no-options --batch --homedir "$expired_home" \
    --faked-system-time '1704067200!' --pinentry-mode loopback --passphrase '' \
    --quick-generate-key 'AROS expired fixture <expired@example.invalid>' \
    rsa2048 sign 1d >/dev/null
expired_fingerprint=$(gpg --no-options --batch --homedir "$expired_home" \
    --with-colons --list-keys --fingerprint | \
    awk -F: '$1 == "fpr" { print toupper($10); exit }')
gpg --no-options --batch --homedir "$expired_home" --armor --no-emit-version \
    --no-comments --export "$expired_fingerprint" > "$work/expired-key.asc"
expect_failure "$root/scripts/release/verify-apt-public-key.sh" \
    --key "$work/expired-key.asc" --fingerprint "$expired_fingerprint" \
    --keyring-output "$work/expired-keyring.gpg"
cat > "$work/expired.status" <<STATUS
[GNUPG:] NEWSIG
[GNUPG:] EXPKEYSIG 0000000000000000 expired
[GNUPG:] VALIDSIG $expired_fingerprint 2024-01-01 1704067200 0 4 0 1 10 00 $expired_fingerprint
STATUS
expect_failure "$root/scripts/release/verify-gpgv-status.sh" \
    --status-file "$work/expired.status" --fingerprint "$expired_fingerprint"

cp -R "$work/apt-a" "$work/apt-invalid-inventory"
: > "$work/apt-invalid-inventory/private-key.asc"
expect_failure "$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-invalid-inventory" --mode full --version 1.2.3 \
    --fingerprint "$apt_fingerprint"
unlink "$work/apt-invalid-inventory/private-key.asc"
unlink "$work/apt-invalid-inventory/dists/stable/InRelease"
expect_failure "$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-invalid-inventory" --mode full --version 1.2.3 \
    --fingerprint "$apt_fingerprint"

mkdir -p "$work/apt-metadata/dists/stable"
install -m 0644 "$work/apt-a/aros-tools-archive-keyring.asc" \
    "$work/apt-metadata/aros-tools-archive-keyring.asc"
for name in Release Release.gpg InRelease; do
    install -m 0644 "$work/apt-a/dists/stable/$name" \
        "$work/apt-metadata/dists/stable/$name"
done
"$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-metadata" --mode metadata \
    --fingerprint "$apt_fingerprint" >/dev/null
printf '\n' >> "$work/apt-metadata/dists/stable/Release"
expect_failure "$root/scripts/release/verify-apt-publication-inventory.sh" \
    --directory "$work/apt-metadata" --mode metadata \
    --fingerprint "$apt_fingerprint"
AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
  "$root/scripts/release/build-apt-repository.sh" \
    --candidate-dir "$work/apt-candidate" \
    --output-dir "$work/apt-refresh" --version 1.2.3 \
    --source-date-epoch 1704672000 \
    --private-key "$work/apt-private.asc" \
    --passphrase-file "$work/apt-passphrase" \
    --fingerprint "$apt_fingerprint"
for path in \
    pool/main/a/aros-tools/aros-tools_1.2.3_amd64.deb \
    pool/main/a/aros-tools/aros-tools_1.2.3_arm64.deb \
    dists/stable/main/binary-amd64/Packages \
    dists/stable/main/binary-amd64/Packages.gz \
    dists/stable/main/binary-arm64/Packages \
    dists/stable/main/binary-arm64/Packages.gz; do
    cmp "$work/apt-a/$path" "$work/apt-refresh/$path"
done
diff --recursive --no-dereference \
    "$work/apt-a/dists/stable/main/binary-amd64/by-hash" \
    "$work/apt-refresh/dists/stable/main/binary-amd64/by-hash"
diff --recursive --no-dereference \
    "$work/apt-a/dists/stable/main/binary-arm64/by-hash" \
    "$work/apt-refresh/dists/stable/main/binary-arm64/by-hash"
if cmp -s "$work/apt-a/dists/stable/InRelease" \
    "$work/apt-refresh/dists/stable/InRelease"; then
    printf '%s\n' 'APT refresh fixture did not advance signed metadata' >&2
    exit 1
fi

# The public mirror verifier consumes every mutable alias, both by-hash forms,
# both packages and the complete signed Release triplet. Its trust anchor is
# exactly one primary key and one signature bound to that primary fingerprint.
AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
  "$root/scripts/release/download-verify-apt-publication.sh" \
    --directory "$work/apt-public-copy" --source-directory "$work/apt-a" \
    --fingerprint "$apt_fingerprint" --version 1.2.3 >/dev/null
diff --recursive --no-dereference "$work/apt-a" "$work/apt-public-copy"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-public-expired" --source-directory "$work/apt-a" \
      --fingerprint "$apt_fingerprint" --version 1.2.3
AROS_RELEASE_POLICY_FIXTURE=1 \
  "$root/scripts/release/download-verify-apt-publication.sh" \
    --directory "$work/apt-public-expired-preflight" \
    --source-directory "$work/apt-a" --fingerprint "$apt_fingerprint" \
    --version 1.2.3 --allow-expired >/dev/null

gpg --batch --faked-system-time '1704067200!' --pinentry-mode loopback \
    --passphrase '' --quick-generate-key \
    'AROS attacker fixture <attacker@example.invalid>' rsa2048 sign 0 >/dev/null
attacker_fingerprint=$(gpg --batch --with-colons --list-secret-keys --fingerprint | \
    awk -F: -v trusted="$apt_fingerprint" \
      '$1 == "fpr" && toupper($10) != trusted { print toupper($10); exit }')
[[ "$attacker_fingerprint" =~ ^[0-9A-F]{40}$ ]] || exit 1
gpg --batch --armor --pinentry-mode loopback --passphrase '' \
    --export-secret-keys "$apt_fingerprint" "$attacker_fingerprint" \
    > "$work/apt-multiple-private.asc"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
    "$root/scripts/release/build-apt-repository.sh" \
      --candidate-dir "$work/apt-candidate" \
      --output-dir "$work/apt-multiple-private-output" --version 1.2.3 \
      --source-date-epoch 1704067200 \
      --private-key "$work/apt-multiple-private.asc" \
      --passphrase-file "$work/apt-passphrase" \
      --fingerprint "$apt_fingerprint"
cp -R "$work/apt-a" "$work/apt-extra-key"
gpg --batch --armor --export "$attacker_fingerprint" \
    >> "$work/apt-extra-key/aros-tools-archive-keyring.asc"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-extra-key-copy" --source-directory "$work/apt-extra-key" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

# One syntactically canonical armor envelope may still carry multiple primary
# keys.  Fingerprint-first validation must reject that form before dearmor.
cp -R "$work/apt-a" "$work/apt-two-primary-one-armor"
gpg --batch --armor --export "$apt_fingerprint" "$attacker_fingerprint" \
    > "$work/apt-two-primary-one-armor/aros-tools-archive-keyring.asc"
[[ $(grep -c '^-----BEGIN PGP PUBLIC KEY BLOCK-----$' \
    "$work/apt-two-primary-one-armor/aros-tools-archive-keyring.asc") == 1 ]] || exit 1
documented_key_fingerprint() {
    gpg --batch --show-keys --with-colons --fingerprint "$1" | awk -F: '
        $1 == "pub" { primary_keys += 1; validity = $2; next }
        $1 == "fpr" && primary_keys == 1 && !fingerprint {
            fingerprint = toupper($10)
        }
        END {
            if (primary_keys != 1 || length(fingerprint) != 40 ||
                fingerprint !~ /^[0-9A-F]+$/ || validity ~ /^[redi]$/) exit 1
            print fingerprint
        }
    '
}
[[ $(documented_key_fingerprint \
    "$work/apt-a/aros-tools-archive-keyring.asc") == "$apt_fingerprint" ]] || exit 1
if documented_key_fingerprint \
    "$work/apt-two-primary-one-armor/aros-tools-archive-keyring.asc" >/dev/null 2>&1; then
    printf '%s\n' 'documented APT key check accepted two primary keys in one armor' >&2
    exit 1
fi
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-two-primary-one-armor-copy" \
      --source-directory "$work/apt-two-primary-one-armor" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

# OpenPGP permits optional Armor headers, but the APT trust-anchor contract has
# one byte-canonical, option-free export.  A semantically identical key with an
# injected Comment header must therefore fail before dearmor.
cp -R "$work/apt-a" "$work/apt-comment-header"
awk 'NR == 1 { print; print "Comment: alternate-representation"; next } { print }' \
    "$work/apt-a/aros-tools-archive-keyring.asc" \
    > "$work/apt-comment-header/aros-tools-archive-keyring.asc"
[[ $(documented_key_fingerprint \
    "$work/apt-comment-header/aros-tools-archive-keyring.asc") == \
    "$apt_fingerprint" ]] || exit 1
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-comment-header-copy" \
      --source-directory "$work/apt-comment-header" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

installation_doc="$root/docs-site/src/content/docs/getting-started/installation.md"
# These are literal documentation markers, not substitutions in this fixture.
# shellcheck disable=SC2016
grep -F 'KEY=$(mktemp)' "$installation_doc" >/dev/null
# shellcheck disable=SC2016
grep -F 'KEYRING=$(mktemp)' "$installation_doc" >/dev/null
grep -F 'primary_keys != 1' "$installation_doc" >/dev/null
grep -F -- '--max-filesize 1048576' "$installation_doc" >/dev/null
grep -F 'command -v sha256sum' "$installation_doc" >/dev/null
grep -F 'shasum -a 256 --check' "$installation_doc" >/dev/null
grep -F -- '--max-filesize 268435456' "$installation_doc" >/dev/null
grep -F -- '--max-filesize 65536' "$installation_doc" >/dev/null
grep -F -- '--max-filesize 4194304' "$installation_doc" >/dev/null
# These are literal documentation markers, not shell substitutions.
# shellcheck disable=SC2016
grep -F 'gpg --no-options --batch --homedir "$KEY_HOME" --armor --no-emit-version' \
    "$installation_doc" >/dev/null
# shellcheck disable=SC2016
grep -F 'cmp "$KEY" "$CANONICAL_KEY"' "$installation_doc" >/dev/null
if grep -Eq '/tmp/aros-tools[^ ]*(key|ring)' "$installation_doc"; then
    printf '%s\n' 'APT installation documentation uses a predictable key path' >&2
    exit 1
fi

# Execute the documented native-install block with hermetic transport and
# verifier commands.  A failed identity verification must stop before either
# archive extraction or the first privileged install command.
native_script="$work/native-install.sh"
awk '
    /^## Native release archive$/ { section = 1; next }
    section && /^```sh$/ { capture = 1; next }
    capture && /^```$/ { exit }
    capture { print }
' "$installation_doc" > "$native_script"
grep -Fx 'set -eu' "$native_script" >/dev/null
# This is a literal documentation marker, not a shell substitution.
# shellcheck disable=SC2016
grep -F 'sudo "$SUITE/aros" install --source-bin "$SUITE" --prefix "$PREFIX"' \
    "$native_script" >/dev/null
if grep -E 'sudo[[:space:]]+install([[:space:]]|$)' "$native_script" >/dev/null; then
    printf '%s\n' 'native installation documentation bypasses the suite transaction' >&2
    exit 1
fi
if grep -F '/bin/*' "$native_script" >/dev/null; then
    printf '%s\n' 'native installation documentation uses a wildcard binary inventory' >&2
    exit 1
fi
native_mock="$work/native-mock-bin"
mkdir "$native_mock"
cat > "$native_mock/curl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
output=
while (($#)); do
    case "$1" in
        --output) output=${2:?}; shift 2 ;;
        *) shift ;;
    esac
done
[[ -n "$output" ]]
: > "$output"
MOCK
cat > "$native_mock/sha256sum" <<'MOCK'
#!/usr/bin/env bash
exit 0
MOCK
cat > "$native_mock/jq" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
MOCK
cat > "$native_mock/gh" <<'MOCK'
#!/usr/bin/env bash
exit 0
MOCK
cat > "$native_mock/cosign" <<'MOCK'
#!/usr/bin/env bash
exit 42
MOCK
for command in tar sudo; do
    cat > "$native_mock/$command" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$0" >> "${NATIVE_SIDE_EFFECT_LOG:?}"
exit 90
MOCK
done
chmod 0755 "$native_mock"/* "$native_script"
expect_failure env PATH="$native_mock:$PATH" \
    NATIVE_SIDE_EFFECT_LOG="$work/native-side-effect" sh "$native_script"
[[ ! -e "$work/native-side-effect" ]] || {
    printf '%s\n' 'native documentation continued after failed identity verification' >&2
    exit 1
}

# With identity verification successful, the documentation must delegate one
# privileged operation to the verified aros suite installer. Its Rust tests own
# exact-inventory, no-clobber, injected-failure and concurrent-race coverage.
cat > "$native_mock/cosign" <<'MOCK'
#!/usr/bin/env bash
exit 0
MOCK
cat > "$native_mock/tar" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
directory=
while (($#)); do
    case "$1" in
        --directory) directory=${2:?}; shift 2 ;;
        *) shift ;;
    esac
done
suite="$directory/aros-tools-v0.1.0-aarch64-apple-darwin/bin"
mkdir -p "$suite"
for binary in aros aros-ahi-runner aros-collect aros-fetch aros-genmodule \
    aros-romtool aros-transpiler aros-verify; do
    printf '%s\n' '#!/bin/sh' 'exit 0' > "$suite/$binary"
    chmod 0755 "$suite/$binary"
done
cat > "$suite/aros" <<'INSTALLER'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" > "${NATIVE_SIDE_EFFECT_LOG:?}"
exit 91
INSTALLER
chmod 0755 "$suite/aros"
MOCK
cat > "$native_mock/sudo" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
exec "$@"
MOCK
chmod 0755 "$native_mock/cosign" "$native_mock/tar" "$native_mock/sudo"
expect_failure env PATH="$native_mock:$PATH" \
    NATIVE_SIDE_EFFECT_LOG="$work/native-installer-invocation" sh "$native_script"
grep -F 'install --source-bin ' "$work/native-installer-invocation" >/dev/null
grep -F -- '--prefix /usr/local' "$work/native-installer-invocation" >/dev/null

# These are literal script markers, not shell substitutions.
# shellcheck disable=SC2016
grep -F -- '--no-comments --export "$export_selector"' \
    "$root/scripts/release/build-apt-repository.sh" >/dev/null
# Without a trailing exclamation mark gpg exports every subkey and picks one
# itself when signing. Both selectors therefore have to be pinned once a domain
# subkey is named, and fall back to the primary otherwise.
# shellcheck disable=SC2016
grep -F -- 'export_selector="${signing_subkey}!"' \
    "$root/scripts/release/build-apt-repository.sh" >/dev/null
# shellcheck disable=SC2016
grep -F -- 'local_user="${signing_subkey}!"' \
    "$root/scripts/release/build-apt-repository.sh" >/dev/null
# shellcheck disable=SC2016
grep -F -- 'export_selector="$fingerprint"' \
    "$root/scripts/release/build-apt-repository.sh" >/dev/null
# The status verifier has to compare the signing subkey, not just the primary,
# or a subkey-to-domain mismatch stays invisible.
# shellcheck disable=SC2016
grep -F -- 'signer = toupper($3)' \
    "$root/scripts/release/verify-gpgv-status.sh" >/dev/null
# Feld 15 der sec-Zeile ist '#', wenn der geheime Primaerschluessel fehlt.
# Diese Pruefung erzwingt "Primaer bleibt offline" maschinell.
# shellcheck disable=SC2016
grep -F -- 'primary_stub = ($15 == "#")' \
    "$root/scripts/release/verify-apt-signing-key.sh" >/dev/null
grep -F -- '--armor --no-emit-version --no-comments' \
    "$root/.github/workflows/refresh-apt-metadata.yml" >/dev/null
grep -F 'verify-apt-public-key.sh' \
    "$root/scripts/release/verify-apt-publication-inventory.sh" >/dev/null
grep -F 'verify-apt-public-key.sh' \
    "$root/scripts/release/verify-apt-recovery-base.sh" >/dev/null
grep -F 'brew install actionlint cmake coreutils cosign curl dpkg gh git' \
    "$root/docs-site/src/content/docs/getting-started/prerequisites.md" >/dev/null

cp -R "$work/apt-a" "$work/apt-missing-release-signature"
unlink "$work/apt-missing-release-signature/dists/stable/Release.gpg"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-missing-release-signature-copy" \
      --source-directory "$work/apt-missing-release-signature" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

cp -R "$work/apt-a" "$work/apt-missing-plain-by-hash"
plain_digest=$("${fixture_checksum[@]}" \
    "$work/apt-missing-plain-by-hash/dists/stable/main/binary-amd64/Packages" | \
    awk '{ print $1 }')
unlink "$work/apt-missing-plain-by-hash/dists/stable/main/binary-amd64/by-hash/SHA256/$plain_digest"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    "$root/scripts/release/download-verify-apt-publication.sh" \
      --directory "$work/apt-missing-plain-by-hash-copy" \
      --source-directory "$work/apt-missing-plain-by-hash" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

# Same-version recovery admits only an exact immutable pool/by-hash base. The
# mutable index aliases, detached signature, or commit point may be missing;
# an invalid present commit point and an extra trust anchor always fail closed.
mkdir "$work/apt-recovery-expected"
AROS_RELEASE_POLICY_FIXTURE=1 AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
  "$root/scripts/release/run-apt-metadata-renderer.sh" \
    "$work/apt-candidate" "$work/apt-recovery-expected" 1.2.3 1704067200
cp -R "$work/apt-a" "$work/apt-recovery-aliases"
unlink "$work/apt-recovery-aliases/dists/stable/main/binary-amd64/Packages"
unlink "$work/apt-recovery-aliases/dists/stable/Release.gpg"
printf '%s\n' 'recoverable divergent alias' \
    > "$work/apt-recovery-aliases/dists/stable/main/binary-arm64/Packages.gz"
recovery_state=$(AROS_RELEASE_POLICY_FIXTURE=1 \
  "$root/scripts/release/verify-apt-recovery-base.sh" \
    --expected-directory "$work/apt-recovery-expected" \
    --source-directory "$work/apt-recovery-aliases" \
    --fingerprint "$apt_fingerprint" --version 1.2.3)
[[ "$recovery_state" == committed ]] || exit 1

cp -R "$work/apt-a" "$work/apt-recovery-no-commit"
unlink "$work/apt-recovery-no-commit/dists/stable/InRelease"
recovery_state=$(AROS_RELEASE_POLICY_FIXTURE=1 \
  "$root/scripts/release/verify-apt-recovery-base.sh" \
    --expected-directory "$work/apt-recovery-expected" \
    --source-directory "$work/apt-recovery-no-commit" \
    --fingerprint "$apt_fingerprint" --version 1.2.3)
[[ "$recovery_state" == missing-commit-point ]] || exit 1

cp -R "$work/apt-a" "$work/apt-recovery-bad-commit"
printf '%s\n' tampered >> "$work/apt-recovery-bad-commit/dists/stable/InRelease"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/verify-apt-recovery-base.sh" \
      --expected-directory "$work/apt-recovery-expected" \
      --source-directory "$work/apt-recovery-bad-commit" \
      --fingerprint "$apt_fingerprint" --version 1.2.3
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 \
    "$root/scripts/release/verify-apt-recovery-base.sh" \
      --expected-directory "$work/apt-recovery-expected" \
      --source-directory "$work/apt-extra-key" \
      --fingerprint "$apt_fingerprint" --version 1.2.3

# Public-state preflight and final verification cover GitHub, signed APT,
# Homebrew and AUR together. Same-version replay requires exact bytes; a newer
# version in any one channel rejects the whole release before exposure.
mkdir -p "$work/channel-candidate" "$work/channels/github-assets" \
    "$work/channels/homebrew/Formula" "$work/channels/aur"
cp "$work/apt-candidate"/*.deb "$work/channel-candidate/"
cp -R "$work/apt-a" "$work/channels/apt"
cat > "$work/channel-candidate/RELEASE_NOTES.md" <<'MARKDOWN'
## 1.2.3

* exact public state fixture
MARKDOWN
cat > "$work/channel-candidate/aros-tools.rb" <<'RUBY'
class ArosTools < Formula
  url "https://example.invalid/releases/download/v1.2.3/a"
  url "https://example.invalid/releases/download/v1.2.3/b"
  url "https://example.invalid/releases/download/v1.2.3/c"
  url "https://example.invalid/releases/download/v1.2.3/d"
end
RUBY
cat > "$work/channel-candidate/PKGBUILD" <<'PKGBUILD'
pkgname=aros-tools-bin
pkgver=1.2.3
pkgrel=1
arch=('x86_64' 'aarch64')
PKGBUILD
cp "$work/channel-candidate/aros-tools.rb" \
    "$work/channels/homebrew/Formula/aros-tools.rb"
cp "$work/channel-candidate/PKGBUILD" "$work/channels/aur/PKGBUILD"
cat > "$work/channels/aur/.SRCINFO" <<'SRCINFO'
pkgbase = aros-tools-bin
	pkgver = 1.2.3
	pkgrel = 1
	arch = x86_64
	arch = aarch64
pkgname = aros-tools-bin
SRCINFO
cat > "$work/channels/aur-rpc.json" <<'JSON'
{"resultcount":1,"results":[{"Name":"aros-tools-bin","Version":"1.2.3-1"}]}
JSON
jq -n --rawfile body "$work/channel-candidate/RELEASE_NOTES.md" \
    '[[{tag_name:"v1.2.3",draft:false,prerelease:false,immutable:true,
        name:"aros-tools v1.2.3",body:$body}]]' \
    > "$work/channels/github-releases.json"
cp "$work/channel-candidate"/* "$work/channels/github-assets/"
cat > "$work/mock-bin/docker" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
cat "${MOCK_SRCINFO:?}"
MOCK
chmod 0755 "$work/mock-bin/docker"
channel_verify=(
    "$root/scripts/release/verify-publication-channels.sh"
    --repository example/project --tag v1.2.3
    --candidate-dir "$work/channel-candidate"
    --apt-base-url https://deb.example.invalid/aros-tools
    --apt-fingerprint "$apt_fingerprint"
    --fixture-root "$work/channels"
)
for verify_mode in preflight exact; do
    AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
      MOCK_SRCINFO="$work/channels/aur/.SRCINFO" GH_TOKEN=fixture \
      PATH="$work/mock-bin:$PATH" \
      "${channel_verify[@]}" --mode "$verify_mode" >/dev/null
done
cp "$work/channels/apt/dists/stable/main/binary-amd64/Packages" \
    "$work/Packages.amd64.saved"
printf '\n' >> "$work/channels/apt/dists/stable/main/binary-amd64/Packages"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    MOCK_SRCINFO="$work/channels/aur/.SRCINFO" GH_TOKEN=fixture \
    PATH="$work/mock-bin:$PATH" \
    "${channel_verify[@]}" --mode exact
mv "$work/Packages.amd64.saved" \
    "$work/channels/apt/dists/stable/main/binary-amd64/Packages"
cp "$work/channels/apt/dists/stable/Release" "$work/Release.saved"
printf '\n' >> "$work/channels/apt/dists/stable/Release"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    MOCK_SRCINFO="$work/channels/aur/.SRCINFO" GH_TOKEN=fixture \
    PATH="$work/mock-bin:$PATH" \
    "${channel_verify[@]}" --mode exact
mv "$work/Release.saved" "$work/channels/apt/dists/stable/Release"
printf '%s\n' \
    '{"resultcount":1,"results":[{"Name":"aros-tools-bin","Version":"9.0.0-1"}]}' \
    > "$work/channels/aur-rpc.json"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    MOCK_SRCINFO="$work/channels/aur/.SRCINFO" GH_TOKEN=fixture \
    PATH="$work/mock-bin:$PATH" \
    "${channel_verify[@]}" --mode preflight
printf '%s\n' \
    '{"resultcount":1,"results":[{"Name":"aros-tools-bin","Version":"1.2.3-1"}]}' \
    > "$work/channels/aur-rpc.json"
printf '%s\n' '# divergent same-version bytes' \
    >> "$work/channels/homebrew/Formula/aros-tools.rb"
expect_failure env AROS_RELEASE_POLICY_FIXTURE=1 AROS_RELEASE_NOW_EPOCH=1704067200 \
    MOCK_SRCINFO="$work/channels/aur/.SRCINFO" GH_TOKEN=fixture \
    PATH="$work/mock-bin:$PATH" \
    "${channel_verify[@]}" --mode preflight
unset GNUPGHOME

# Snapshot-CAS is a checked workflow contract: all seven mutable objects are
# captured before the first write, mutation helpers never perform a fresh HEAD,
# refresh shares the five protected environments, and final verification uses
# the complete public mirror. Exercise both the live positive policy and narrow
# negative mutations.
"$root/scripts/release/verify-apt-workflow-contract.sh" "$root" >/dev/null
mkdir -p "$work/apt-workflow-contract/.github/workflows"
cp "$root/.github/workflows/publish-ecosystem.yml" \
    "$work/apt-workflow-contract/.github/workflows/publish-ecosystem.yml"
cp "$root/.github/workflows/refresh-apt-metadata.yml" \
    "$work/apt-workflow-contract/.github/workflows/refresh-apt-metadata.yml"
"$root/scripts/release/verify-apt-workflow-contract.sh" \
    "$work/apt-workflow-contract" >/dev/null
python3 - "$work/apt-workflow-contract/.github/workflows/publish-ecosystem.yml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text()
needle = '          publish_snapshot() {\n'
if text.count(needle) != 1:
    raise SystemExit('fixture cannot locate singular publish_snapshot function')
path.write_text(text.replace(
    needle,
    needle + '            aws s3api head-object --bucket unsafe --key unsafe\n',
    1,
))
PY
expect_failure "$root/scripts/release/verify-apt-workflow-contract.sh" \
    "$work/apt-workflow-contract"
cp "$root/.github/workflows/publish-ecosystem.yml" \
    "$work/apt-workflow-contract/.github/workflows/publish-ecosystem.yml"
sed -i.bak 's/environment: apt-signing/environment: apt-refresh-signing/' \
    "$work/apt-workflow-contract/.github/workflows/refresh-apt-metadata.yml"
rm "$work/apt-workflow-contract/.github/workflows/refresh-apt-metadata.yml.bak"
expect_failure "$root/scripts/release/verify-apt-workflow-contract.sh" \
    "$work/apt-workflow-contract"

# The workflow trust policy itself must reject mutable external actions.
mkdir -p "$work/policy/.github/workflows"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: bad
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@main
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
cat > "$work/policy/.github/workflows/arbitrary.yml" <<'YAML'
name: credential-persisting-checkout-in-any-workflow
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
YAML
unlink "$work/policy/.github/workflows/bad.yml"
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
unlink "$work/policy/.github/workflows/arbitrary.yml"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: mutable-shell-container
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - run: docker run --rm archlinux:latest true
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: variable-shell-container
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - run: docker run --rm "$IMAGE" true
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: weak-attestation
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - run: |
          gh attestation verify subject --repo example/project
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: weak-transport
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - run: curl --fail --location https://example.invalid/artifact
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"
cat > "$work/policy/.github/workflows/bad.yml" <<'YAML'
name: weak-short-transport
on: push
jobs:
  bad:
    runs-on: ubuntu-24.04
    steps:
      - run: curl -fL https://example.invalid/artifact
YAML
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"

cat > "$work/policy/.github/workflows/publish-ecosystem.yml" <<'YAML'
name: mixed-secret-domains
on: workflow_call
jobs:
  apt-sign:
    runs-on: ubuntu-24.04
    env:
      PRIVATE_KEY: ${{ secrets.APT_GPG_PRIVATE_KEY }}
      R2_KEY: ${{ secrets.R2_ACCESS_KEY_ID }}
    steps:
      - name: signed-apt-publication
        run: |
          trap cleanup EXIT
          trap 'exit 130' HUP INT TERM
          gpgconf --kill gpg-agent
  apt:
    runs-on: ubuntu-24.04
    env:
      R2_KEY: ${{ secrets.R2_ACCESS_KEY_ID }}
    steps:
      - name: signed-apt-publication
        run: |
          trap cleanup EXIT
          trap 'exit 130' HUP INT TERM
          gpgconf --kill gpg-agent
  aur-publish:
    runs-on: ubuntu-24.04
    env:
      AUR_KEY: ${{ secrets.AUR_SSH_PRIVATE_KEY }}
    steps:
      - name: aur-publication-evidence
        run: |
          trap cleanup EXIT
          trap 'exit 130' HUP INT TERM
          gpgconf --kill gpg-agent
  aur-verify:
    runs-on: ubuntu-24.04
    steps:
      - name: aur-publication-evidence
        run: true
YAML
expect_failure_matching 'combines private credential domains' \
    "$root/scripts/release/check-actions-policy.sh" "$work/policy"
unlink "$work/policy/.github/workflows/publish-ecosystem.yml"

# Release preflights are bound to their exact protected environments, while
# channel inspection and the aggregator stay credential free.
rm -f "$work/policy/.github/workflows"/*
cp "$root/.github/workflows/release.yml" \
    "$work/policy/.github/workflows/release.yml"
"$root/scripts/release/check-actions-policy.sh" "$work/policy" >/dev/null
sed -i.bak \
    '/^  homebrew-credential-preflight:/,/^  r2-credential-preflight:/ s/environment: homebrew-publication/environment: release/' \
    "$work/policy/.github/workflows/release.yml"
rm "$work/policy/.github/workflows/release.yml.bak"
expect_failure "$root/scripts/release/check-actions-policy.sh" "$work/policy"

printf '%s\n' 'release policy fixtures passed'
