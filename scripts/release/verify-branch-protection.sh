#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7030 %s\n' "$*" >&2
    exit 1
}

repository=
contract=
while (($#)); do
    case "$1" in
        --repository) repository=${2:-}; shift 2 ;;
        --contract) contract=${2:-}; shift 2 ;;
        *) fail "unknown branch-protection verifier argument: $1" ;;
    esac
done

for command_name in gh jq python3; do
    command -v "$command_name" >/dev/null || fail "required command is missing: $command_name"
done
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || \
    fail 'repository must be an owner/repository name'
[[ -n "${GH_TOKEN:-}" ]] || fail 'GH_TOKEN is required for protection checks'

script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
repository_root=$(unset CDPATH; cd -- "$script_root/../.." && pwd -P)
contract=${contract:-"$repository_root/contracts/repository-governance-v1.toml"}
[[ -f "$contract" ]] || fail "governance contract does not exist: $contract"

expected=$(python3 - "$contract" "$repository" <<'PY'
import json
import pathlib
import sys
import tomllib

path = pathlib.Path(sys.argv[1])
repository = sys.argv[2]
with path.open('rb') as stream:
    contract = tomllib.load(stream)
if contract.get('schema_version') != 1:
    raise SystemExit('unsupported repository governance schema')
try:
    policy = contract['repositories'][repository]
except KeyError as error:
    raise SystemExit(f'governance contract has no policy for {repository}') from error
required = {
    'branch',
    'required_approving_review_count',
    'dismiss_stale_reviews',
    'require_code_owner_reviews',
    'require_last_push_approval',
    'required_conversation_resolution',
    'enforce_admins',
    'required_linear_history',
    'allow_force_pushes',
    'allow_deletions',
    'required_status_checks',
}
if set(policy) != required:
    raise SystemExit(
        f'governance policy fields differ: expected {sorted(required)}, got {sorted(policy)}'
    )
checks = policy['required_status_checks']
if not checks:
    raise SystemExit('governance policy must require at least one status check')
contexts = [check.get('context') for check in checks]
if any(not isinstance(context, str) or not context for context in contexts):
    raise SystemExit('governance status-check context is malformed')
if len(contexts) != len(set(contexts)):
    raise SystemExit('governance status-check contexts are not unique')
if any(not isinstance(check.get('app_id'), int) or check['app_id'] <= 0 for check in checks):
    raise SystemExit('every governance status check requires a positive app_id')
approval_count = policy['required_approving_review_count']
if type(approval_count) is not int or not 0 <= approval_count <= 6:
    raise SystemExit('governance approval count must be an integer from zero through six')
if approval_count == 0 and policy['require_last_push_approval']:
    raise SystemExit('last-push approval cannot be required when approvals are disabled')
print(json.dumps(policy, sort_keys=True, separators=(',', ':')))
PY
) || fail 'repository governance contract is invalid'

branch=$(jq -er '.branch' <<<"$expected") || fail 'governance branch is missing'
[[ "$branch" =~ ^[A-Za-z0-9._/-]+$ ]] || fail 'governance branch is malformed'
branch_state=$(gh api "repos/${repository}/branches/${branch}") || \
    fail "cannot read ${repository}/${branch} branch state"
[[ $(jq -r '.protected' <<<"$branch_state") == true ]] || \
    fail "${repository}/${branch} is not protected"

protection=$(gh api "repos/${repository}/branches/${branch}/protection") || \
    fail "cannot read effective protection for ${repository}/${branch}"
expected_checks=$(jq -c \
    '[.required_status_checks[] | {context, app_id}] | sort_by(.context)' \
    <<<"$expected")
actual_checks=$(jq -c \
    '[.required_status_checks.checks[]? | {context, app_id}] | sort_by(.context)' \
    <<<"$protection")
if [[ "$actual_checks" != "$expected_checks" ]]; then
    fail "${repository}/${branch} status checks differ from the closed governance contract"
fi

if ! jq -e --argjson policy "$expected" '
    .required_status_checks.strict == true and
    .required_pull_request_reviews != null and
    .required_pull_request_reviews.required_approving_review_count ==
        $policy.required_approving_review_count and
    .required_pull_request_reviews.dismiss_stale_reviews ==
        $policy.dismiss_stale_reviews and
    .required_pull_request_reviews.require_code_owner_reviews ==
        $policy.require_code_owner_reviews and
    .required_pull_request_reviews.require_last_push_approval ==
        $policy.require_last_push_approval and
    .required_conversation_resolution.enabled ==
        $policy.required_conversation_resolution and
    .enforce_admins.enabled == $policy.enforce_admins and
    .required_linear_history.enabled == $policy.required_linear_history and
    .allow_force_pushes.enabled == $policy.allow_force_pushes and
    .allow_deletions.enabled == $policy.allow_deletions
' <<<"$protection" >/dev/null; then
    fail "${repository}/${branch} protection differs from the closed governance contract"
fi

printf '%s\n' "$(jq -r '.commit.sha' <<<"$branch_state")"
