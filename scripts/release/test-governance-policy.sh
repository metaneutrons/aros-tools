#!/usr/bin/env bash

set -euo pipefail

script_root=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd -P)
repository_root=$(unset CDPATH; cd -- "$script_root/../.." && pwd -P)
work=$(mktemp -d)
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT HUP INT TERM

repository=metaneutrons/homebrew-tap
mkdir "$work/bin"
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'case "$1:$2" in' \
    '  api:repos/metaneutrons/homebrew-tap/branches/main) cat "$BRANCH_FIXTURE" ;;' \
    '  api:repos/metaneutrons/homebrew-tap/branches/main/protection) cat "$PROTECTION_FIXTURE" ;;' \
    '  *) exit 90 ;;' \
    'esac' > "$work/bin/gh"
chmod 0755 "$work/bin/gh"
printf '%s\n' \
    '{"protected":true,"commit":{"sha":"0123456789abcdef0123456789abcdef01234567"}}' \
    > "$work/branch.json"

write_baseline() {
    python3 - "$repository_root/contracts/repository-governance-v1.toml" \
        "$repository" "$work/protection.json" <<'PY'
import json
import sys
import tomllib

with open(sys.argv[1], 'rb') as stream:
    policy = tomllib.load(stream)['repositories'][sys.argv[2]]
payload = {
    'required_status_checks': {'strict': True, 'checks': policy['required_status_checks']},
    'required_pull_request_reviews': {
        'required_approving_review_count': policy['required_approving_review_count'],
        'dismiss_stale_reviews': policy['dismiss_stale_reviews'],
        'require_code_owner_reviews': policy['require_code_owner_reviews'],
        'require_last_push_approval': policy['require_last_push_approval'],
    },
    'required_conversation_resolution': {
        'enabled': policy['required_conversation_resolution'],
    },
    'enforce_admins': {'enabled': policy['enforce_admins']},
    'required_linear_history': {'enabled': policy['required_linear_history']},
    'allow_force_pushes': {'enabled': policy['allow_force_pushes']},
    'allow_deletions': {'enabled': policy['allow_deletions']},
}
with open(sys.argv[3], 'w', encoding='utf-8') as stream:
    json.dump(payload, stream)
PY
}

run_verifier() {
    PATH="$work/bin:$PATH" GH_TOKEN=fixture \
        BRANCH_FIXTURE="$work/branch.json" \
        PROTECTION_FIXTURE="$work/protection.json" \
        "$script_root/verify-branch-protection.sh" \
        --repository "$repository" >/dev/null
}

mutate_and_reject() {
    local label=$1 expression=$2
    write_baseline
    jq "$expression" "$work/protection.json" > "$work/protection.next"
    mv "$work/protection.next" "$work/protection.json"
    if run_verifier 2>/dev/null; then
        printf 'governance verifier accepted invalid tap policy: %s\n' "$label" >&2
        exit 1
    fi
}

write_baseline
run_verifier
mutate_and_reject approvals-enabled \
    '.required_pull_request_reviews.required_approving_review_count = 1'
mutate_and_reject last-push-enabled \
    '.required_pull_request_reviews.require_last_push_approval = true'
mutate_and_reject conversations-disabled \
    '.required_conversation_resolution.enabled = false'
mutate_and_reject missing-app-id \
    'del(.required_status_checks.checks[0].app_id)'
mutate_and_reject wrong-app-id \
    '.required_status_checks.checks[0].app_id = 1'

printf '%s\n' 'repository governance policy fixtures passed'
