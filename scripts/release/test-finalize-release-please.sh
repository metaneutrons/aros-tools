#!/usr/bin/env bash

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/aros-release-please-finalize.XXXXXX")
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT

cat > "$work/gh" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "${MOCK_LOG:?}"
case " $* " in
    *'/releases/tags/v1.2.3 '*) printf '%s\n' "${MOCK_RELEASE:?}" ;;
    *'/commits/0123456789abcdef0123456789abcdef01234567/pulls?per_page=100 '*)
        printf '%s\n' "${MOCK_PULLS:?}" ;;
    *' --method DELETE repos/example/tools/issues/42/labels/autorelease%3A%20pending '*)
        printf '%s\n' '{}' ;;
    *'/issues/42 '*) printf '%s\n' "${MOCK_ISSUE:?}" ;;
    *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 91 ;;
esac
MOCK
chmod 0755 "$work/gh"

release='{"tag_name":"v1.2.3","name":"aros-tools v1.2.3","draft":false,"prerelease":false,"immutable":true}'
pending_pr='[[{"number":42,"state":"closed","merged_at":"2026-09-21T00:00:00Z","merge_commit_sha":"0123456789abcdef0123456789abcdef01234567","base":{"ref":"main"},"head":{"ref":"release-please--branches--main--components--aros-tools"},"title":"chore(main): release 1.2.3","user":{"login":"app/metaneutrons-release-please"},"labels":[{"name":"autorelease: pending"}]}]]'
complete_pr='[[{"number":42,"state":"closed","merged_at":"2026-09-21T00:00:00Z","merge_commit_sha":"0123456789abcdef0123456789abcdef01234567","base":{"ref":"main"},"head":{"ref":"release-please--branches--main--components--aros-tools"},"title":"chore(main): release 1.2.3","user":{"login":"app/metaneutrons-release-please"},"labels":[]}]]'

run() {
    MOCK_LOG="$work/log" MOCK_RELEASE="$release" MOCK_PULLS="$1" MOCK_ISSUE="$2" \
      GH_TOKEN=fixture GITHUB_REPOSITORY=example/tools TAG=v1.2.3 \
      SOURCE_COMMIT=0123456789abcdef0123456789abcdef01234567 \
      PATH="$work:$PATH" "$root/scripts/release/finalize-release-please.sh"
}

: > "$work/log"
run "$pending_pr" '{"number":42,"labels":[]}' >/dev/null
grep -F 'api --method DELETE repos/example/tools/issues/42/labels/autorelease%3A%20pending' \
    "$work/log" >/dev/null || {
    printf '%s\n' 'pending Release Please label was not removed' >&2
    exit 1
}

: > "$work/log"
run "$complete_pr" '{"number":42,"labels":[]}' >/dev/null
if grep -Fq -- '--method DELETE' "$work/log"; then
    printf '%s\n' 'completed Release Please lifecycle was not idempotent' >&2
    exit 1
fi

: > "$work/log"
if run '[]' '{"number":42,"labels":[]}' >"$work/stdout" 2>"$work/stderr"; then
    printf '%s\n' 'unrelated pull request fixture unexpectedly finalized' >&2
    exit 1
fi
grep -F 'AP7520 exact Release Please PR cannot be resolved' "$work/stderr" >/dev/null || {
    cat "$work/stderr" >&2
    exit 1
}
if grep -Fq -- '--method DELETE' "$work/log"; then
    printf '%s\n' 'unrelated pull request fixture mutated labels' >&2
    exit 1
fi

printf '%s\n' 'Release Please finalization fixtures passed'
