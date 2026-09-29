#!/usr/bin/env bash

set -euo pipefail

root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/aros-homebrew-openssl.XXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/mock-bin" "$fixture/prefix/bin"

# The mock's environment variables must expand when it runs, not while this
# fixture writes the mock program.
# shellcheck disable=SC2016
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'case "$1" in' \
    '  --prefix) printf "%s\n" "$TEST_BREW_PREFIX" ;;' \
    '  unlink)' \
    '    [[ "$2" == openssl@1.1 ]] || exit 2' \
    '    printf "unlink %s\n" "$2" >> "$TEST_BREW_CALLS"' \
    '    [[ "${TEST_UNLINK_FAIL:-0}" == 0 ]] || exit 1' \
    '    rm -- "$TEST_BREW_PREFIX/bin/openssl" ;;' \
    '  *) exit 2 ;;' \
    'esac' > "$fixture/mock-bin/brew"
chmod +x "$fixture/mock-bin/brew"

export PATH="$fixture/mock-bin:$PATH"
export TEST_BREW_PREFIX="$fixture/prefix"
export TEST_BREW_CALLS="$fixture/brew-calls"
script="$root/scripts/release/prepare-homebrew-openssl.sh"

# Positive case: unlink only the observed legacy OpenSSL owner.
ln -s "$TEST_BREW_PREFIX/opt/openssl@1.1/bin/openssl" \
    "$TEST_BREW_PREFIX/bin/openssl"
bash "$script" aarch64-apple-darwin
[[ ! -e "$TEST_BREW_PREFIX/bin/openssl" && ! -L "$TEST_BREW_PREFIX/bin/openssl" ]]
[[ $(<"$TEST_BREW_CALLS") == 'unlink openssl@1.1' ]]

# An unrelated link must survive untouched, as must a non-macOS host.
ln -s "$TEST_BREW_PREFIX/opt/openssl@3/bin/openssl" \
    "$TEST_BREW_PREFIX/bin/openssl"
bash "$script" aarch64-apple-darwin
bash "$script" x86_64-unknown-linux-gnu
[[ $(readlink "$TEST_BREW_PREFIX/bin/openssl") == \
    "$TEST_BREW_PREFIX/opt/openssl@3/bin/openssl" ]]
[[ $(<"$TEST_BREW_CALLS") == 'unlink openssl@1.1' ]]
rm -- "$TEST_BREW_PREFIX/bin/openssl"

# Counter-probe: a failed unlink is fatal and leaves the conflict visible.
ln -s "$TEST_BREW_PREFIX/opt/openssl@1.1/bin/openssl" \
    "$TEST_BREW_PREFIX/bin/openssl"
export TEST_UNLINK_FAIL=1
if bash "$script" aarch64-apple-darwin; then
    echo 'failed legacy unlink unexpectedly qualified the runner' >&2
    exit 1
fi
[[ -L "$TEST_BREW_PREFIX/bin/openssl" ]]
[[ $(<"$TEST_BREW_CALLS") == $'unlink openssl@1.1\nunlink openssl@1.1' ]]

printf '%s\n' 'Homebrew OpenSSL runner-preparation probes passed'
