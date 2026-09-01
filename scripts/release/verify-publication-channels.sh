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
apt_base_url=
apt_fingerprint=
fixture_root=
while (($#)); do
    case "$1" in
        --mode) mode=${2:-}; shift 2 ;;
        --repository) repository=${2:-}; shift 2 ;;
        --tag) tag=${2:-}; shift 2 ;;
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        --apt-base-url) apt_base_url=${2:-}; shift 2 ;;
        --apt-fingerprint) apt_fingerprint=${2:-}; shift 2 ;;
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
[[ "$apt_base_url" == https://* && "$apt_base_url" != */ ]] || fail 'APT base URL is unsafe'
[[ "$apt_fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || fail 'APT fingerprint is malformed'
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

fetch_optional() {
    local relative=$1 output=$2 class=$3 status
    local arguments=(--output "$output" --class "$class" --allow-not-found)
    if [[ -n "$fixture_root" ]]; then
        arguments+=(--source-file "$fixture_root/apt/$relative")
    else
        arguments+=(--url "$apt_base_url/$relative")
    fi
    set +e
    AROS_RELEASE_POLICY_FIXTURE="${AROS_RELEASE_POLICY_FIXTURE:-}" \
        "$root/scripts/release/download-bounded-https.sh" "${arguments[@]}"
    status=$?
    set -e
    case "$status" in
        0) return 0 ;;
        44) return 1 ;;
        *) fail "bounded APT request failed for $relative" ;;
    esac
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

# Signed APT state. Exact mode requires the complete public mirror. Preflight
# additionally accepts a same-version state whose immutable pool/by-hash base
# is exact but whose mutable aliases need controlled repair.
if fetch_optional dists/stable/InRelease "$work/apt-InRelease-probe" apt-release; then
    apt_public="$work/apt-public"
    apt_error="$work/apt-public.error"
    if [[ -n "$fixture_root" ]]; then
        source_args=(--source-directory "$fixture_root/apt")
    else
        source_args=(--base-url "$apt_base_url")
    fi
    if apt_version=$("$root/scripts/release/download-verify-apt-publication.sh" \
        --directory "$apt_public" "${source_args[@]}" \
        --fingerprint "$apt_fingerprint" --version auto 2> "$apt_error"); then
        :
    elif [[ "$mode" == preflight ]]; then
        rm -rf -- "$apt_public"
        if apt_version=$("$root/scripts/release/download-verify-apt-publication.sh" \
            --directory "$apt_public" "${source_args[@]}" \
            --fingerprint "$apt_fingerprint" --version auto --allow-expired \
            2> "$apt_error"); then
            :
        else
            rm -rf -- "$apt_public"
            apt_expected="$work/apt-expected"
            install -d "$apt_expected"
            metadata_epoch=${AROS_RELEASE_NOW_EPOCH:-$(date +%s)}
            if [[ -n "$fixture_root" ]]; then
                AROS_APT_RENDER_LOCAL_FOR_TESTS=1 \
                  "$root/scripts/release/run-apt-metadata-renderer.sh" \
                    "$candidate_dir" "$apt_expected" "$version" "$metadata_epoch"
            else
                "$root/scripts/release/run-apt-metadata-renderer.sh" \
                    "$candidate_dir" "$apt_expected" "$version" "$metadata_epoch"
            fi
            if ! recovery_state=$("$root/scripts/release/verify-apt-recovery-base.sh" \
                --expected-directory "$apt_expected" "${source_args[@]}" \
                --fingerprint "$apt_fingerprint" --version "$version"); then
                cat "$apt_error" >&2
                fail 'APT channel is neither complete nor safely recoverable at the requested version'
            fi
            [[ "$recovery_state" == committed ]] || \
                fail 'APT recovery preflight lost its signed commit point'
            apt_version=$version
        fi
    else
        cat "$apt_error" >&2
        fail 'APT channel has not converged as a complete public inventory'
    fi
    apt_relation=$(require_acceptable_version APT "$apt_version")
    if [[ "$apt_relation" == same ]]; then
        for arch in amd64 arm64; do
            if [[ -d "$apt_public" ]]; then
                public_deb="$apt_public/pool/main/a/aros-tools/aros-tools_${version}_${arch}.deb"
            else
                public_deb="$candidate_dir/aros-tools_${version}_${arch}.deb"
            fi
            cmp "$candidate_dir/aros-tools_${version}_${arch}.deb" "$public_deb" || \
                fail "same-version APT $arch package bytes differ"
        done
    fi
elif [[ "$mode" == exact ]]; then
    fail 'APT channel has not converged'
fi

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
