---
title: Installation
description: Build the complete tools suite from source and make it available to your AROS checkouts.
---

:::caution[Beta availability]
Build from source today. The first stable native archives and package channels
are still pending qualification. Check [release status](/aros-tools/reference/release-status/)
before using any package command below.
:::

## Requirements

Start with the [host prerequisites](/aros-tools/getting-started/prerequisites/):
Rust, Cargo, Git and a native compiler/linker. CMake, Ninja, Python, curl and
patch are also used by the AROS workflows.

You do not need the complete contributor audit environment just to build and
try the suite. That environment is documented under
[development](/aros-tools/contributing/development/).

## Build from source

Run this in the directory where you keep source checkouts:

```sh
git clone https://github.com/metaneutrons/aros-tools.git
cd aros-tools
cargo build --release --workspace --all-features --locked
```

Keep the eight public executables together in `target/release`. The workspace
also produces the internal `aros-release` program; it is not part of a user
installation.

## Make the suite available

For this terminal session, while still in the tools checkout:

```sh
export PATH="$PWD/target/release:$PATH"
aros --version
aros build-tools check
```

The check probes the six helpers required by CMake and verifies that their
versions match the frontend. The seventh companion, `aros-verify`, is used
for independent verification and is also part of the installed suite.

For future sessions, add the **absolute** `target/release` path to your shell's
PATH configuration. Do not put a relative path there: you will run `aros`
from a separate operating-system checkout.

Next: [create your first checkout and build](/aros-tools/getting-started/quick-start/).

## Native release archive

After a release is qualified, use its target-matched archive. The following
procedure verifies the checksum and signing identity before extracting and
installing. Select the actual published version; `0.1.0` below illustrates
the version format.

<details>
<summary>Verified archive installation procedure (for a published release)</summary>

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

</details>

## Debian and Ubuntu

The package channel is `https://deb.metaneutrons.cc`.
After it is marked available, verify the archive key before adding the source.

<details>
<summary>Signed APT installation procedure (after channel qualification)</summary>

The central metaneutrons archive signs the repository, not the tools project.
Its primary fingerprint is
`1B7B79417383648BBFBE282E01AB8296EF0FCD76`; the domain signing subkey is
`A0C21782FC507CCBD666F3ED242072FEC8BE54A4`.

Verify both before installing the domain keyring:

```sh
set -eu
WORK=$(mktemp -d)
cleanup() { trap - EXIT; rm -rf -- "$WORK"; }
trap cleanup EXIT
trap 'exit 130' HUP INT TERM
mkdir -m 0700 "$WORK/gnupg"
KEY="$WORK/metaneutrons-archive-keyring.pgp"
PRIMARY=1B7B79417383648BBFBE282E01AB8296EF0FCD76
SIGNING_FINGERPRINT=A0C21782FC507CCBD666F3ED242072FEC8BE54A4
curl --fail --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --max-filesize 1048576 \
  https://deb.metaneutrons.cc/metaneutrons-archive-keyring.pgp --output "$KEY"
IDENTITY=$(gpg --no-options --batch --no-autostart --homedir "$WORK/gnupg" \
  --show-keys --with-colons --fingerprint "$KEY" | awk -F: '
  $1 == "pub" { primary += 1; if ($2 ~ /^[redi]$/ || $12 !~ /c/ || $12 ~ /s/) bad = 1 }
  $1 == "sub" { subkey += 1; if ($2 ~ /^[redi]$/ || $12 !~ /s/) bad = 1 }
  $1 == "sec" || $1 == "ssb" { bad = 1 }
  $1 == "fpr" { print $10; count += 1 }
  END { if (bad || primary != 1 || subkey != 1 || count != 2) exit 1 }
')
test "$IDENTITY" = "$(printf '%s\n%s' "$PRIMARY" "$SIGNING_FINGERPRINT")"
sudo install -m 0644 "$KEY" /usr/share/keyrings/metaneutrons-archive-keyring.pgp
sudo tee /etc/apt/sources.list.d/aros-tools.sources >/dev/null <<'SOURCES'
Types: deb
URIs: https://deb.metaneutrons.cc
Suites: rolling
Components: main
Architectures: amd64 arm64
Signed-By: /usr/share/keyrings/metaneutrons-archive-keyring.pgp
SOURCES
sudo apt-get update
sudo apt-get install aros-tools
```

Only `amd64` and `arm64` are published. APT authenticates the signed release and
package index, including content-addressed by-hash downloads. Do not add
`trusted=yes`, disable expiry checks or globally trust the key.

</details>

## Homebrew

After the formula is publicly qualified:

```sh
brew install metaneutrons/tap/aros-tools
brew test metaneutrons/tap/aros-tools
```

## Arch Linux (AUR)

After the package is publicly qualified, review its `PKGBUILD` and install
with your usual AUR workflow. For example:

```sh
paru -S aros-tools-bin
```

See [package channels](/aros-tools/reference/publication/) for supported hosts
and provenance, or [update and uninstall](/aros-tools/getting-started/update-uninstall/)
for an existing installation.
