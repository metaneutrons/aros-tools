---
title: Installation
description: Install aros-tools from reviewed source or, after public qualification, from one measured native release payload.
---

## Requirements

- Rust 1.98.0 with Cargo (selected automatically by `rust-toolchain.toml`)
- Git
- CMake and Ninja for the optional translated build engine
- Node.js 24 or newer with npm, actionlint, and ShellCheck for the complete local gate
- the host packages required by the selected AROS target

The verified binary paths below have additional end-user prerequisites. The
native archive path uses curl, `jq`, GitHub CLI (`gh`), cosign and `tar`
and either `sha256sum` or `shasum`. The Debian/Ubuntu path uses curl, GnuPG
(`gpg` and `gpgconf`) and `dpkg`. Install these first using the concrete
[host instructions in Prerequisites](/aros-tools/getting-started/prerequisites/).

## Build from source

```sh
git clone https://github.com/metaneutrons/aros-tools.git
cd aros-tools
cargo build --release --workspace --all-features --locked
```

The executables are written to `target/release`. Keep the eight public tools
together: the `aros` frontend resolves build tools as separate processes so
their command, logging and failure boundaries remain independently testable.
The workspace also builds the internal `aros-release` qualification producer;
do not install it as part of the user-facing suite.

Before installing a development build, install the pinned audit helpers once
and run the same canonical workspace gate used by CI:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-machete --version 0.9.2 --locked
AROS_TEST_SOURCE_ROOT=/absolute/path/to/qualified/AROS-NX \
  scripts/check-workspace.sh
```

The script is the workspace-gate single source of truth. Its default `all`
mode includes formatting, architecture and Actions policy, actionlint,
ShellCheck, locked strict Clippy, locked rustdoc, audit, deny, machete, the
locked Astro build, and locked workspace tests. CI uses the closed
source-independent `portable-test` suite on all
four supported hosts and reserves the recursive exact-source `test` gate for
one Linux lane. That lane also builds the ordinary workspace executables,
discovers every CMake-engine fixture and runs each host-compatible fixture
against the same source identity; `cmake` and `ninja` are required. The real
GRUB host-build fixture is an explicit Darwin/arm64 release qualification and
is visibly omitted on other hosts. The default local `all` contract remains
complete. The
separate documentation workflow calls `docs` directly and uploads only its
verified output through a separately pinned Pages action.

Setting `AROS_TEST_SOURCE_ROOT` opts into a real source-init/sync/transpiler
integration test. The named checkout is only read; the test works in a separate
temporary clone. Set `AROS_TEST_TOOLS_DIR` only when the six build-tool
executables come from a prebuilt directory instead of Cargo's target directory.

The complete workspace gate currently uses the immutable AROS-NX source
contract named by CI. Tests for components which already support pristine
upstream remain part of that gate; a complete upstream-only product build is a
separate release criterion and is not implied by this command.

:::caution[Pre-release availability]
The commands below are the closed installation contract, not a claim that a
package is already public. Use them only after the
[release-status page](/aros-tools/reference/release-status/) links a fully
qualified release. An unavailable URL is not permission to use an unofficial
mirror or omit verification.
:::

## Native release archive

Choose the exact version and target from the GitHub release. Each archive has a
checksum, manifest, SPDX SBOM, Sigstore bundle and GitHub attestation:

```sh
set -eu
VERSION=0.1.0
TARGET=aarch64-apple-darwin # choose one supported target
BASE="https://github.com/metaneutrons/aros-tools/releases/download/v${VERSION}"
ARCHIVE="aros-tools-v${VERSION}-${TARGET}.tar.gz"
PREFIX=${PREFIX:-/usr/local}
WORK=$(mktemp -d)
EXTRACT="$WORK/extracted"
cleanup() { trap - EXIT; rm -rf -- "$WORK"; }
trap cleanup EXIT
trap 'exit 130' HUP INT TERM
mkdir "$EXTRACT"

curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --max-filesize 268435456 --output "$WORK/$ARCHIVE" "$BASE/$ARCHIVE"
curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --max-filesize 65536 --output "$WORK/$ARCHIVE.sha256" "$BASE/$ARCHIVE.sha256"
curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --max-filesize 4194304 --output "$WORK/$ARCHIVE.manifest.json" "$BASE/$ARCHIVE.manifest.json"
curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --max-filesize 4194304 --output "$WORK/$ARCHIVE.sigstore.json" "$BASE/$ARCHIVE.sigstore.json"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$WORK" && sha256sum --check "$ARCHIVE.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$WORK" && shasum -a 256 --check "$ARCHIVE.sha256")
else
  echo 'error: sha256sum or shasum is required' >&2
  exit 1
fi
SOURCE_COMMIT=$(jq -er .source_commit "$WORK/$ARCHIVE.manifest.json")
gh attestation verify "$WORK/$ARCHIVE" \
  --repo metaneutrons/aros-tools \
  --signer-workflow metaneutrons/aros-tools/.github/workflows/release.yml \
  --source-ref "refs/tags/v${VERSION}" \
  --source-digest "$SOURCE_COMMIT" \
  --deny-self-hosted-runners
cosign verify-blob \
  --bundle "$WORK/$ARCHIVE.sigstore.json" \
  --certificate-identity "https://github.com/metaneutrons/aros-tools/.github/workflows/release.yml@refs/tags/v${VERSION}" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  "$WORK/$ARCHIVE"

tar --extract --gzip --file "$WORK/$ARCHIVE" --directory "$EXTRACT"
SUITE="$EXTRACT/aros-tools-v${VERSION}-${TARGET}/bin"
case "$PREFIX" in
  /*) ;;
  *) echo 'error: PREFIX must be an absolute path' >&2; exit 1 ;;
esac
sudo "$SUITE/aros" install --source-bin "$SUITE" --prefix "$PREFIX"
"$PREFIX/bin/aros" --version
cleanup
trap - EXIT HUP INT TERM
```

The installer validates the exact eight-file inventory and executable modes,
then publishes the suite through one locked, crash-recoverable no-clobber
transaction. It never changes the mode of an existing `bin` directory and
never replaces an existing program. `aros` intentionally calls its specialized
executables as separate processes, so mixed versions are unsupported. For an
existing installation, follow [Update and uninstall](/aros-tools/getting-started/update-uninstall/)
instead of overwriting individual files.

## Debian and Ubuntu

The signed repository lives below `https://deb.metaneutrons.cc/aros-tools`.
Verify the archive key fingerprint before installing it:

```sh
set -eu
BASE=https://deb.metaneutrons.cc/aros-tools
EXPECTED_FINGERPRINT=D69E2F2FD93F55BD0EB3D02224DA82C3E25C0392
KEY=$(mktemp)
KEYRING=$(mktemp)
CANONICAL_KEY=$(mktemp)
KEY_HOME=$(mktemp -d)
chmod 0700 "$KEY_HOME"
cleanup() {
  trap - EXIT
  gpgconf --homedir "$KEY_HOME" --kill gpg-agent >/dev/null 2>&1 || true
  rm -rf -- "$KEY_HOME"
  rm -f -- "$KEY" "$KEYRING" "$CANONICAL_KEY"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM
curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --max-filesize 1048576 \
  "$BASE/aros-tools-archive-keyring.asc" --output "$KEY"
test "$(head -n 1 "$KEY")" = '-----BEGIN PGP PUBLIC KEY BLOCK-----'
test "$(tail -n 1 "$KEY")" = '-----END PGP PUBLIC KEY BLOCK-----'
test "$(grep -c '^-----BEGIN PGP PUBLIC KEY BLOCK-----$' "$KEY")" -eq 1
test "$(grep -c '^-----END PGP PUBLIC KEY BLOCK-----$' "$KEY")" -eq 1
FINGERPRINT=$(gpg --no-options --batch --show-keys --with-colons --fingerprint "$KEY" | awk -F: '
  $1 == "pub" { primary_keys += 1; validity = $2; next }
  $1 == "fpr" && primary_keys == 1 && !fingerprint {
    fingerprint = toupper($10)
  }
  END {
    if (primary_keys != 1 || length(fingerprint) != 40 ||
        fingerprint !~ /^[0-9A-F]+$/ || validity ~ /^[redi]$/) exit 1
    print fingerprint
  }
')
test "$FINGERPRINT" = "$EXPECTED_FINGERPRINT"
gpg --no-options --batch --homedir "$KEY_HOME" --import "$KEY" >/dev/null 2>&1
gpg --no-options --batch --homedir "$KEY_HOME" --armor --no-emit-version \
  --no-comments --export "$FINGERPRINT" > "$CANONICAL_KEY"
cmp "$KEY" "$CANONICAL_KEY" >/dev/null
gpg --no-options --batch --homedir "$KEY_HOME" --yes --dearmor \
  --output "$KEYRING" "$KEY"
sudo install -m 0644 "$KEYRING" /usr/share/keyrings/aros-tools-archive-keyring.gpg
cleanup
trap - EXIT HUP INT TERM
printf 'deb [arch=%s signed-by=/usr/share/keyrings/aros-tools-archive-keyring.gpg] %s stable main\n' \
  "$(dpkg --print-architecture)" "$BASE" | \
  sudo tee /etc/apt/sources.list.d/aros-tools.list >/dev/null
sudo apt-get update
sudo apt-get install aros-tools
```

Only `amd64` and `arm64` are published. APT authenticates the signed release and
package index; do not add `trusted=yes` or globally trust the key.

## Homebrew

```sh
brew install metaneutrons/tap/aros-tools
brew test metaneutrons/tap/aros-tools
```

The tap formula selects one of the four measured GitHub archives and verifies
its SHA-256.

## Arch Linux (AUR)

Review the public `PKGBUILD` and install `aros-tools-bin` with your normal AUR
workflow, for example:

```sh
paru -S aros-tools-bin
```

The AUR package supports `x86_64` and `aarch64` and selects the corresponding
measured archive; it does not rebuild different release bytes.
