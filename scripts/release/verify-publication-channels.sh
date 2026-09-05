#!/usr/bin/env bash

# Verify the four public release states before exposure or after convergence.
# Preflight accepts an absent/older channel and an exact same-version replay;
# exact mode requires every channel to contain the candidate bytes.

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)

fail() {
    printf '::error::AP7090 %s\n' "$*" >&2
    exit 1
}

mode=
repository=
tag=
candidate_dir=
fixture_root=
while (($#)); do
    case "$1" in
        --mode) mode=${2:-}; shift 2 ;;
        --repository) repository=${2:-}; shift 2 ;;
        --tag) tag=${2:-}; shift 2 ;;
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        --fixture-root) fixture_root=${2:-}; shift 2 ;;
        *) fail "unknown channel verifier argument: $1" ;;
    esac
done

[[ "$mode" == preflight || "$mode" == exact ]] || fail 'mode must be preflight or exact'
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail 'repository is malformed'
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'stable tag is malformed'
version=${tag#v}
[[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || fail 'candidate directory is unsafe'
[[ -z $(find "$candidate_dir" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'candidate directory contains a non-regular entry'
[[ -f "$candidate_dir/RELEASE_NOTES.md" && ! -L "$candidate_dir/RELEASE_NOTES.md" ]] || \
    fail 'signed release notes are missing'
if [[ -n "$fixture_root" ]]; then
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'fixtures are permitted only in release-policy tests'
    [[ -d "$fixture_root" && ! -L "$fixture_root" ]] || fail 'fixture root is unsafe'
fi
[[ -n "${GH_TOKEN:-}" || -n "$fixture_root" ]] || fail 'GH_TOKEN is required'

work=$(mktemp -d "${TMPDIR:-/tmp}/aros-publication-state.XXXXXX")
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT HUP INT TERM

version_relation() {
    python3 - "$1" "$2" <<'PY'
import re
import sys

pattern = re.compile(r'^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$')
values = []
for value in sys.argv[1:]:
    match = pattern.fullmatch(value)
    if match is None:
        raise SystemExit(f'malformed stable version: {value}')
    values.append(tuple(int(part) for part in match.groups()))
print('older' if values[0] < values[1] else 'newer' if values[0] > values[1] else 'same')
PY
}

require_acceptable_version() {
    local channel=$1 current=$2 relation
    relation=$(version_relation "$current" "$version") || fail "$channel version is malformed"
    case "$relation" in
        newer) fail "$channel already exposes newer version $current; refusing downgrade to $version" ;;
        older)
            [[ "$mode" == preflight ]] || \
                fail "$channel has not converged: $current instead of $version"
            ;;
        same) ;;
        *) fail "$channel version comparison failed" ;;
    esac
    printf '%s\n' "$relation"
}

compare_directories() {
    local expected=$1 actual=$2 label=$3
    [[ -d "$actual" && ! -L "$actual" ]] || fail "$label asset directory is unsafe"
    [[ -z $(find "$actual" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
        fail "$label assets contain a non-regular entry"
    local expected_names="$work/${label}-expected" actual_names="$work/${label}-actual"
    find "$expected" -mindepth 1 -maxdepth 1 -type f -exec basename {} \; | \
        LC_ALL=C sort > "$expected_names"
    find "$actual" -mindepth 1 -maxdepth 1 -type f -exec basename {} \; | \
        LC_ALL=C sort > "$actual_names"
    diff -u "$expected_names" "$actual_names" || fail "$label asset names differ"
    while IFS= read -r name; do
        cmp "$expected/$name" "$actual/$name" || fail "$label asset differs: $name"
    done < "$expected_names"
}

# GitHub public release state.
if [[ -n "$fixture_root" ]]; then
    cp "$fixture_root/github-releases.json" "$work/releases.json"
else
    gh api --paginate --slurp \
        "repos/${repository}/releases?per_page=100" > "$work/releases.json"
fi
jq -e 'type == "array" and all(.[]; type == "array")' "$work/releases.json" >/dev/null || \
    fail 'GitHub release history is malformed'
jq -c '[.[][] | select(.draft == false and .prerelease == false)]' \
    "$work/releases.json" > "$work/stable-releases.json"
duplicate_tags=$(jq -r 'group_by(.tag_name)[] | select(length != 1) | .[0].tag_name' \
    "$work/stable-releases.json")
[[ -z "$duplicate_tags" ]] || fail "GitHub release history has duplicate tags: $duplicate_tags"
github_same=$(jq -c --arg tag "$tag" '[.[] | select(.tag_name == $tag)]' \
    "$work/stable-releases.json")
[[ $(jq 'length' <<<"$github_same") -le 1 ]] || fail 'GitHub release identity is ambiguous'
github_versions=$(jq -r '.[].tag_name // empty' "$work/stable-releases.json" | \
    sed -n -E 's/^v([0-9]+\.[0-9]+\.[0-9]+)$/\1/p')
if [[ -n "$github_versions" ]]; then
    while IFS= read -r published_version; do
        require_acceptable_version GitHub "$published_version" >/dev/null
    done <<< "$github_versions"
fi
if [[ $(jq 'length' <<<"$github_same") == 1 ]]; then
    jq -e --arg title "aros-tools $tag" \
        --rawfile body "$candidate_dir/RELEASE_NOTES.md" \
        '.[0].immutable == true and .[0].name == $title and .[0].body == $body' \
        <<<"$github_same" >/dev/null || fail 'same-version GitHub release metadata is not exact and immutable'
    if [[ -n "$fixture_root" ]]; then
        compare_directories "$candidate_dir" "$fixture_root/github-assets" github
    else
        "$root/scripts/release/verify-release-assets.sh" \
            --repository "$repository" --tag "$tag" --candidate-dir "$candidate_dir" >/dev/null
    fi
elif [[ "$mode" == exact ]]; then
    fail 'GitHub release has not converged'
fi

# Central APT is a read-only consumer boundary. Signing, metadata recovery and
# retention belong to apt-archive; the tools only accept its signed, exact bytes.
apt_args=(--mode "$mode" --version "$version" --candidate-dir "$candidate_dir")
if [[ -n "$fixture_root" ]]; then
    apt_args+=(--fixture-root "$fixture_root/apt" --contract "$fixture_root/apt-contract.toml")
fi
python3 "$root/scripts/release/verify-central-apt.py" "${apt_args[@]}" >/dev/null

# Public Homebrew tap state.
if [[ -n "$fixture_root" ]]; then
    cp -R "$fixture_root/homebrew" "$work/homebrew"
else
    git -c http.followRedirects=false clone --quiet --depth 1 --branch main --single-branch \
        https://github.com/metaneutrons/homebrew-tap.git "$work/homebrew"
fi
formula="$work/homebrew/Formula/aros-tools.rb"
if [[ -e "$formula" && ( ! -f "$formula" || -L "$formula" ) ]]; then
    fail 'Homebrew formula path is unsafe'
fi
if [[ -f "$formula" && ! -L "$formula" ]]; then
    homebrew_version=$(grep -Eo '/releases/download/v[0-9]+\.[0-9]+\.[0-9]+/' "$formula" | \
        sed -E 's#^.*/v([0-9]+\.[0-9]+\.[0-9]+)/$#\1#' | LC_ALL=C sort -u)
    [[ "$homebrew_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        fail 'Homebrew formula has no singular stable version'
    homebrew_relation=$(require_acceptable_version Homebrew "$homebrew_version")
    if [[ "$homebrew_relation" == same ]]; then
        cmp "$candidate_dir/aros-tools.rb" "$formula" || \
            fail 'same-version Homebrew formula bytes differ'
    fi
elif [[ "$mode" == exact ]]; then
    fail 'Homebrew channel has not converged'
fi

# Public AUR RPC and Git state.
if [[ -n "$fixture_root" ]]; then
    AROS_RELEASE_POLICY_FIXTURE=1 \
        "$root/scripts/release/download-bounded-https.sh" \
        --source-file "$fixture_root/aur-rpc.json" --output "$work/aur-rpc.json" \
        --class json
else
    "$root/scripts/release/download-bounded-https.sh" \
        --url 'https://aur.archlinux.org/rpc?v=5&type=info&arg%5B%5D=aros-tools-bin' \
        --output "$work/aur-rpc.json" --class json
fi
aur_count=$(jq -r '.resultcount' "$work/aur-rpc.json")
[[ "$aur_count" =~ ^[01]$ ]] || fail 'AUR RPC result is ambiguous'
if [[ -n "$fixture_root" ]]; then
    cp -R "$fixture_root/aur" "$work/aur"
else
    git -c http.followRedirects=false clone --quiet --depth 1 --branch master --single-branch \
        https://aur.archlinux.org/aros-tools-bin.git "$work/aur"
fi
if [[ -e "$work/aur/PKGBUILD" && \
      ( ! -f "$work/aur/PKGBUILD" || -L "$work/aur/PKGBUILD" ) ]]; then
    fail 'AUR PKGBUILD path is unsafe'
fi
git_version=
if [[ -f "$work/aur/PKGBUILD" ]]; then
    git_version=$(sed -n -E "s/^pkgver=['\"]?([0-9]+\.[0-9]+\.[0-9]+)['\"]?$/\1/p" \
        "$work/aur/PKGBUILD")
    [[ "$git_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        fail 'AUR Git version is malformed'
fi
aur_version=$git_version
if [[ "$aur_count" == 1 ]]; then
    aur_version=$(jq -r '.results[0].Version' "$work/aur-rpc.json" | \
        sed -n -E 's/^([0-9]+\.[0-9]+\.[0-9]+)-1$/\1/p')
    [[ "$aur_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'AUR version is malformed'
    [[ "$git_version" == "$aur_version" ]] || fail 'AUR RPC and Git versions disagree'
fi
if [[ -n "$aur_version" ]]; then
    aur_relation=$(require_acceptable_version AUR "$aur_version")
    if [[ "$aur_relation" == same ]]; then
        [[ -f "$work/aur/.SRCINFO" && ! -L "$work/aur/.SRCINFO" ]] || \
            fail 'same-version AUR SRCINFO is missing or unsafe'
        cmp "$candidate_dir/PKGBUILD" "$work/aur/PKGBUILD" || \
            fail 'same-version AUR PKGBUILD bytes differ'
        install -d "$work/srcinfo"
        install -m 0644 "$candidate_dir/PKGBUILD" "$work/srcinfo/PKGBUILD"
        docker run --rm --network none --user "$(id -u):$(id -g)" --env HOME=/tmp \
            --volume "$work/srcinfo:/pkg" --workdir /pkg \
            archlinux:base-devel@sha256:a26046b7363dad8e2614858f4313949ae9b05c9c5f31de343a54864b9e20806f \
            makepkg --printsrcinfo > "$work/generated.SRCINFO"
        cmp "$work/generated.SRCINFO" "$work/aur/.SRCINFO" || \
            fail 'same-version AUR SRCINFO bytes differ'
    fi
fi
if [[ "$mode" == exact && "$aur_count" != 1 ]]; then
    fail 'AUR channel has not converged'
fi

printf '%s\n' "verified $mode policy across GitHub, APT, Homebrew and AUR for $tag"
