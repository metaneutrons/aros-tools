#!/bin/sh
# gitleaks nur über die Commits, die tatsächlich gepusht werden.
# Ein voller History-Scan bei jedem Push wird mit wachsendem Repository
# unbrauchbar langsam und erzieht zu --no-verify.
set -eu

command -v gitleaks >/dev/null 2>&1 || {
    printf '\033[31mPush abgelehnt:\033[0m gitleaks fehlt (brew install gitleaks).\n' >&2
    exit 1
}

zero='0000000000000000000000000000000000000000'

# git reicht dem pre-push-Hook je Ref eine Zeile auf stdin:
#   <lokaler ref> <lokale oid> <entfernter ref> <entfernte oid>
# Das ist die einzige verlässliche Quelle. HEAD und @{upstream} sind es nicht:
# bei `git push origin anderer-branch` oder einem Multi-Ref-Push zeigen sie auf
# den falschen Bereich.
#
# Gefragt ist, was dieser Push bei origin neu einbringt, also
#   <alle lokalen Spitzen> --not <alle Stände, die dort schon liegen>
# in genau dieser Reihenfolge. `--not` wirkt positionsabhängig auf alles
# Folgende; einzelne `a..b`-Bereiche zu mischen wäre falsch, weil ein späteres
# `--not` die früheren Bereiche mit ausschlösse.
tips=''
excludes=''
lines=0
while read -r _local_ref local_oid _remote_ref remote_oid; do
    [ -n "${local_oid:-}" ] || continue
    lines=$((lines + 1))
    # Eine Löschung bringt keine neuen Objekte mit.
    [ "$local_oid" = "$zero" ] && continue
    tips="$tips $local_oid"
    # Der Stand der Gegenseite aus stdin ist maßgeblich. Die lokalen
    # Remote-Tracking-Refs kommen zusätzlich dazu, weil ein Commit auch über
    # einen anderen Branch schon dort liegen kann.
    [ "$remote_oid" != "$zero" ] && excludes="$excludes $remote_oid"
done

if [ "$lines" -gt 0 ]; then
    if [ -z "$tips" ]; then
        printf 'gitleaks: nur Löschungen, keine neuen Objekte.\n' >&2
        exit 0
    fi
    opts="$tips --not $excludes --remotes=origin"
else
    # Kein stdin, etwa beim Aufruf von Hand oder unter einem Runner, der die
    # Refs nicht durchreicht.
    if git rev-parse --verify --quiet HEAD >/dev/null 2>&1; then
        opts="HEAD --not --remotes=origin"
    else
        printf 'gitleaks: kein Commit vorhanden.\n' >&2
        exit 0
    fi
fi

# shellcheck disable=SC2086  # opts ist eine bewusst wortgetrennte Optionsliste.
count=$(git rev-list --count $opts 2>/dev/null || echo 0)
if [ "$count" -eq 0 ]; then
    printf 'gitleaks: keine neuen Commits.\n' >&2
    exit 0
fi

printf 'gitleaks: prüfe %s neue Commits.\n' "$count" >&2
# shellcheck disable=SC2086
exec gitleaks git --redact --verbose --log-opts="$opts" .
