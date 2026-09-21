#!/usr/bin/env bash

# Mark the exact Release Please PR complete only after the caller has proven a
# final immutable stable release and every public channel. Release Please uses
# this label as its sole state machine for opening the next release PR.

set -euo pipefail

fail() {
    printf '::error::AP7520 %s\n' "$*" >&2
    exit 1
}

repository=${GITHUB_REPOSITORY:-}
tag=${TAG:-}
source_commit=${SOURCE_COMMIT:-}
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail 'repository is malformed'
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'stable tag is malformed'
[[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || fail 'source commit is malformed'
[[ -n "${GH_TOKEN:-}" ]] || fail 'GitHub token is required'

version=${tag#v}
release=$(gh api "repos/${repository}/releases/tags/${tag}")
jq -e --arg tag "$tag" --arg title "aros-tools $tag" '
    .tag_name == $tag and .name == $title and .draft == false and
    .prerelease == false and (.immutable // false) == true
' <<<"$release" >/dev/null || fail 'final immutable stable release identity differs'

pulls=$(gh api --paginate --slurp \
    "repos/${repository}/commits/${source_commit}/pulls?per_page=100")
pr=$(jq -cer --arg commit "$source_commit" --arg title "chore(main): release ${version}" '
    [.[][] | select(
        .state == "closed" and
        (.merged_at | type == "string" and length > 0) and
        .merge_commit_sha == $commit and
        .base.ref == "main" and
        .title == $title and
        .user.login == "app/metaneutrons-release-please" and
        (.head.ref | test("^release-please--[A-Za-z0-9._-]+$"))
    )] |
    if length == 1 then .[0] else error("expected exactly one matching Release Please PR") end
' <<<"$pulls") || fail 'exact Release Please PR cannot be resolved'

number=$(jq -er '.number | select(type == "number" and . > 0)' <<<"$pr") || \
    fail 'Release Please PR number is malformed'
pending=$(jq -r '[.labels[].name] | index("autorelease: pending") != null' <<<"$pr") || \
    fail 'Release Please PR labels are malformed'
[[ "$pending" == true || "$pending" == false ]] || fail 'Release Please pending-label state is malformed'
if [[ "$pending" == true ]]; then
    gh api --method DELETE \
        "repos/${repository}/issues/${number}/labels/autorelease%3A%20pending" >/dev/null
fi

state=$(gh api "repos/${repository}/issues/${number}")
jq -e --argjson number "$number" '
    .number == $number and ([.labels[].name] | index("autorelease: pending") | not)
' <<<"$state" >/dev/null || fail 'Release Please pending label remains after finalization'

printf 'finalized Release Please lifecycle for PR #%s and %s\n' "$number" "$tag"
