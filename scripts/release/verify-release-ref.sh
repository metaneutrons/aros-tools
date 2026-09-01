#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7020 %s\n' "$*" >&2
    exit 1
}

repository=
tag=
tag_object=
source_commit=
tag_date_epoch=
governance_contract=

while (($#)); do
    case "$1" in
        --repository) repository=${2:-}; shift 2 ;;
        --tag) tag=${2:-}; shift 2 ;;
        --tag-object) tag_object=${2:-}; shift 2 ;;
        --source-commit) source_commit=${2:-}; shift 2 ;;
        --tag-date-epoch) tag_date_epoch=${2:-}; shift 2 ;;
        --governance-contract) governance_contract=${2:-}; shift 2 ;;
        *) fail "unknown release-reference verifier argument: $1" ;;
    esac
done

for command in gh jq python3; do
    command -v "$command" >/dev/null || fail "required command is missing: $command"
done

[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || \
    fail 'repository must be an owner/repository name'
[[ "$tag" =~ ^v[0-9A-Za-z.+-]+$ ]] || fail 'release tag is malformed'
[[ "$tag_object" =~ ^[0-9a-f]{40}$ ]] || fail 'tag object must be a full Git SHA-1'
[[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || \
    fail 'source commit must be a full Git SHA-1'
[[ "$tag_date_epoch" =~ ^[1-9][0-9]*$ ]] || \
    fail 'tag timestamp must be a positive epoch'
[[ -n "${GH_TOKEN:-}" ]] || fail 'GH_TOKEN is required for remote identity checks'
if [[ -n "$governance_contract" ]]; then
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'governance-contract override is permitted only in release-policy tests'
    [[ -f "$governance_contract" && ! -L "$governance_contract" ]] || \
        fail 'governance-contract override is unsafe'
fi

ref_json=$(gh api "repos/${repository}/git/ref/tags/${tag}")
remote_type=$(jq -r '.object.type' <<<"$ref_json")
remote_tag_object=$(jq -r '.object.sha' <<<"$ref_json")
if [[ "$remote_type" != tag || "$remote_tag_object" != "$tag_object" ]]; then
    fail 'remote release tag is not the qualified annotated tag object'
fi

tag_json=$(gh api "repos/${repository}/git/tags/${remote_tag_object}")
remote_source_type=$(jq -r '.object.type' <<<"$tag_json")
remote_source_commit=$(jq -r '.object.sha' <<<"$tag_json")
if [[ "$remote_source_type" != commit || "$remote_source_commit" != "$source_commit" ]]; then
    fail 'annotated release tag does not point directly to the qualified source commit'
fi
remote_tag_date=$(jq -er '.tagger.date' <<<"$tag_json") || \
    fail 'annotated release tag has no tagger timestamp'
remote_tag_epoch=$(python3 - "$remote_tag_date" <<'PY'
import datetime
import sys

try:
    value = datetime.datetime.fromisoformat(sys.argv[1].replace('Z', '+00:00'))
except ValueError as error:
    raise SystemExit(f'invalid annotated tag timestamp: {error}')
if value.tzinfo is None:
    raise SystemExit('annotated tag timestamp has no timezone')
print(int(value.timestamp()))
PY
)
[[ "$remote_tag_epoch" == "$tag_date_epoch" ]] || \
    fail 'remote annotated tag timestamp differs from the qualified timestamp'

ruleset_pages=$(gh api --paginate --slurp \
    "repos/${repository}/rulesets?includes_parents=true&targets=tag&per_page=100")
ruleset_ids=()
while IFS= read -r ruleset_id; do
    ruleset_ids+=("$ruleset_id")
done < <(jq -r '.[][] | select(.target == "tag" and .enforcement == "active") | .id' \
    <<<"$ruleset_pages")
immutable_ruleset=false
for ruleset_id in "${ruleset_ids[@]}"; do
    [[ "$ruleset_id" =~ ^[1-9][0-9]*$ ]] || fail 'tag ruleset has a malformed ID'
    ruleset=$(gh api "repos/${repository}/rulesets/${ruleset_id}")
    if jq -e '
        .target == "tag" and
        .enforcement == "active" and
        ((.bypass_actors // []) | length == 0) and
        ((.conditions.ref_name.include // []) | index("refs/tags/v*") != null) and
        ((.conditions.ref_name.exclude // []) | length == 0) and
        ([.rules[]?.type] | index("update") != null) and
        ([.rules[]?.type] | index("deletion") != null)
    ' <<<"$ruleset" >/dev/null; then
        immutable_ruleset=true
        break
    fi
done
[[ "$immutable_ruleset" == true ]] || \
    fail 'no active v* tag ruleset forbids both update and deletion'

protection_arguments=(--repository "$repository")
[[ -z "$governance_contract" ]] || \
    protection_arguments+=(--contract "$governance_contract")
main_commit=$("$(dirname -- "$0")/verify-branch-protection.sh" \
    "${protection_arguments[@]}") || \
    fail 'protected main does not match repository governance contract'
[[ "$main_commit" =~ ^[0-9a-f]{40}$ ]] || fail 'protected main has no valid commit identity'

if [[ "$main_commit" != "$source_commit" ]]; then
    comparison=$(gh api \
        "repos/${repository}/compare/${source_commit}...${main_commit}" \
        --jq '.status')
    if [[ "$comparison" != ahead ]]; then
        fail "qualified source is not reachable from protected main (status: $comparison)"
    fi
fi

printf '%s\n' "$main_commit"
