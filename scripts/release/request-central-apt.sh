#!/usr/bin/env bash

# One narrow control-plane request. No signing key, R2 credential or producer
# script crosses the repository boundary. A run ID comes from the dispatch API,
# never from a latest-run search that could attach to somebody else's request.
set -Eeuo pipefail
fail() { printf '::error::AP7252 %s\n' "$*" >&2; exit 1; }
phase='read local consumer contract'
trap 'fail "central archive request failed during: $phase; inspect the preceding error without retrying a possibly submitted dispatch"' ERR
mode=${1:-}
[[ "$mode" == preflight || "$mode" == dispatch ]] || fail 'expected preflight or dispatch'
[[ $# == 1 ]] || fail 'unexpected request argument'
[[ -n "${GH_TOKEN:-}" ]] || fail 'a scoped archive Actions token is required'
script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
contract=$(python3 "$script_root/central-apt-contract.py")
repository=$(jq -er .repository <<<"$contract")
workflow=$(jq -er .workflow <<<"$contract")
domain=$(jq -er .domain <<<"$contract")
project=$(jq -er .project <<<"$contract")
phase='verify protected archive branch'
branch=$(gh api "repos/${repository}/branches/main")
head=$(jq -er '.commit.sha | select(test("^[0-9a-f]{40}$"))' <<<"$branch")
jq -e '.name == "main" and .protected == true' <<<"$branch" >/dev/null || \
    fail 'central archive main must be protected'
work=$(mktemp -d "${TMPDIR:-/tmp}/aros-archive-request.XXXXXX")
trap 'unset GH_TOKEN; rm -rf -- "$work"' EXIT
trap 'exit 130' HUP INT TERM
phase='verify central archive manifest and workflow'
gh api -H 'Accept: application/vnd.github.raw+json' \
    "repos/${repository}/contents/domains/${domain}/manifest.toml?ref=${head}" \
    > "$work/manifest.toml"
python3 "$script_root/central-apt-contract.py" --manifest "$work/manifest.toml" >/dev/null
state=$(gh api "repos/${repository}/actions/workflows/${workflow}")
workflow_id=$(jq -er '.id | select(type == "number" and . > 0)' <<<"$state")
jq -e --arg path ".github/workflows/${workflow}" \
    '.state == "active" and .path == $path' <<<"$state" >/dev/null || \
    fail 'central archive publication workflow is not active at its expected path'
printf 'central archive contract verified at %s\n' "$head" >&2
[[ "$mode" == dispatch ]] || exit 0
phase='recheck archive source before dispatch'
[[ $(gh api "repos/${repository}/commits/main" --jq .sha) == "$head" ]] || \
    fail 'central archive main changed before dispatch; rerun qualification'
# Explicitly select the API version that defines return_run_details. A missing
# response is ambiguous: fail rather than submit the request a second time.
phase='submit one workflow dispatch (response may be ambiguous)'
response=$(gh api --method POST -H 'X-GitHub-Api-Version: 2022-11-28' \
    "repos/${repository}/actions/workflows/${workflow}/dispatches" \
    -f ref=main -F return_run_details=true \
    -f "inputs[domain]=$domain" -f "inputs[project]=$project")
run_id=$(jq -er '.workflow_run_id | select(type == "number" and . > 0)' <<<"$response") || \
    fail 'dispatch returned no exact run ID; inspect archive runs before any retry'
expected_url="https://github.com/${repository}/actions/runs/${run_id}"
jq -e --arg url "$expected_url" '.html_url == $url' <<<"$response" >/dev/null || \
    fail 'dispatch response names an unexpected repository or run URL'
printf 'central archive request: %s\n' "$expected_url" >&2
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '\nCentral APT publication: [%s](%s), source %s.\n' \
        "$run_id" "$expected_url" "$head" >> "$GITHUB_STEP_SUMMARY"
fi
# The App token lasts one hour; publication is bounded to 45 minutes centrally.
# Leave time for a queued run while never waiting indefinitely.
for ((attempt=1; attempt<=165; attempt++)); do
    phase="observe exact archive run $run_id"
    run=$(gh api "repos/${repository}/actions/runs/${run_id}")
    jq -e --arg sha "$head" --argjson workflow "$workflow_id" \
        --argjson id "$run_id" --arg repo "$repository" \
        '.id == $id and .workflow_id == $workflow and .head_sha == $sha and
         .head_branch == "main" and .event == "workflow_dispatch" and
         .repository.full_name == $repo' <<<"$run" >/dev/null || \
        fail 'central archive run identity differs from the verified dispatch'
    if [[ $(jq -r .status <<<"$run") == completed ]]; then
        [[ $(jq -r .conclusion <<<"$run") == success ]] || \
            fail "central archive publication failed; inspect $expected_url"
        printf '%s\n' "$run_id"
        exit 0
    fi
    sleep 20
done
fail "central archive run did not finish within 55 minutes: $expected_url; inspect it before recovery"
